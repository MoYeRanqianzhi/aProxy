//! 查的是仓库与发布资产的公开事实，而不是对软件安全性的判断。没有摘要匹配时，两者甚至不一定属于同一作者。
use aproxy_ipc::desktop::*;
use reqwest::{Client, StatusCode, header};
use serde::{Deserialize, de::DeserializeOwned};
use std::time::Duration;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
type Failure = (Unreachable, Option<u16>);
fn transport(e: reqwest::Error) -> Failure {
    (
        if e.is_timeout() {
            Unreachable::Timeout
        } else {
            Unreachable::Network
        },
        e.status().map(|v| v.as_u16()),
    )
}
async fn get<T: DeserializeOwned>(
    client: &Client,
    url: String,
    cap: usize,
) -> Result<Option<T>, Failure> {
    let mut r = client.get(url).send().await.map_err(transport)?;
    let code = r.status();
    if code == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let http = (Unreachable::Http, Some(code.as_u16()));
    if !code.is_success() {
        return Err(
            if code == StatusCode::TOO_MANY_REQUESTS
                || (code == StatusCode::FORBIDDEN
                    && r.headers()
                        .get("x-ratelimit-remaining")
                        .is_some_and(|h| h == "0"))
            {
                (Unreachable::RateLimited, http.1)
            } else {
                http
            },
        );
    }
    if r.content_length().is_some_and(|n| n > cap as u64) {
        return Err(http);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = r.chunk().await.map_err(transport)? {
        if chunk.len() > cap - bytes.len() {
            return Err(http);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map(Some).map_err(|_| http)
}
#[derive(Deserialize)]
struct Repo {
    full_name: String,
    html_url: String,
    stargazers_count: u64,
    forks_count: u64,
    created_at: String,
    pushed_at: String,
    archived: bool,
    fork: bool,
    license: Option<License>,
}
#[derive(Deserialize)]
struct License {
    spdx_id: Option<String>,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    name: String,
    digest: Option<String>,
    browser_download_url: String,
}
fn timestamp(s: &str) -> Option<i64> {
    (OffsetDateTime::parse(s, &Rfc3339)
        .ok()?
        .unix_timestamp_nanos()
        / 1_000_000)
        .try_into()
        .ok()
}
fn link(s: &str) -> bool {
    url::Url::parse(s).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host().is_some()
            && u.username().is_empty()
            && u.password().is_none()
    })
}
async fn release(
    client: &Client,
    base: &str,
    version: &str,
    source: &PackageSource,
) -> Result<ReleaseFacts, Failure> {
    let PackageSource::File { name, sha256, .. } = source else {
        return Ok(ReleaseFacts::NotAFile);
    };
    for tag in [format!("v{version}"), version.into()] {
        let Some(r) =
            get::<Release>(client, format!("{base}/releases/tags/{tag}"), 8 << 20).await?
        else {
            continue;
        };
        if r.tag_name != tag {
            return Err((Unreachable::Http, Some(200)));
        }
        let hash = format!("sha256:{sha256}");
        if let Some(a) = r.assets.iter().find(|a| a.digest.as_deref() == Some(&hash)) {
            if !link(&a.browser_download_url) {
                return Err((Unreachable::Http, Some(200)));
            }
            return Ok(ReleaseFacts::Match {
                tag,
                asset: super::manifest::clean(&a.name),
                url: a.browser_download_url.clone(),
            });
        }
        if let Some(a) = r
            .assets
            .iter()
            .find(|a| a.name == *name && a.digest.is_some())
        {
            return Ok(ReleaseFacts::Mismatch {
                tag,
                asset: super::manifest::clean(&a.name),
            });
        }
        return Ok(if r.assets.iter().any(|a| a.digest.is_none()) {
            ReleaseFacts::NoDigest { tag }
        } else {
            ReleaseFacts::NoAsset { tag }
        });
    }
    Ok(ReleaseFacts::NoRelease)
}
/// 从给定 API 根读取事实。窗口固定使用 api.github.com；参数让离线测试复用相同的请求、代理和限额逻辑。
pub async fn facts(
    repository: Option<&str>,
    version: &str,
    source: &PackageSource,
    api: &str,
    proxy: Option<&str>,
    timeout: Duration,
) -> GithubFacts {
    let Some(url) = repository else {
        return GithubFacts::NoRepository;
    };
    let Some(name) = url.strip_prefix("https://github.com/") else {
        return GithubFacts::NotGithub { url: url.into() };
    };
    let query = async {
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::ACCEPT,
            header::HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            "x-github-api-version",
            header::HeaderValue::from_static("2022-11-28"),
        );
        let mut builder = Client::builder()
            .user_agent(concat!("aproxy-desktop/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::custom(|a| {
                if a.previous().len() < 10
                    && a.previous()
                        .first()
                        .is_some_and(|u| u.origin() == a.url().origin())
                {
                    a.follow()
                } else {
                    a.stop()
                }
            }));
        if let Some(p) = proxy {
            builder = builder.proxy(reqwest::Proxy::all(p).map_err(transport)?);
        }
        let client = builder.build().map_err(transport)?;
        let base = format!("{api}/repos/{name}");
        let Some(r) = get::<Repo>(&client, base.clone(), 1 << 20).await? else {
            return Ok(GithubFacts::RepoNotFound);
        };
        let invalid = (Unreachable::Http, Some(200));
        if !link(&r.html_url) {
            return Err(invalid);
        }
        let repo = RepoFacts {
            renamed: !r.full_name.eq_ignore_ascii_case(name),
            full_name: super::manifest::clean(&r.full_name),
            url: r.html_url,
            stars: r.stargazers_count.min(SafeUint::MAX),
            forks: r.forks_count.min(SafeUint::MAX),
            created_at: timestamp(&r.created_at).ok_or(invalid)?,
            pushed_at: timestamp(&r.pushed_at).ok_or(invalid)?,
            archived: r.archived,
            is_fork: r.fork,
            license: r
                .license
                .and_then(|l| l.spdx_id)
                .map(|s| super::manifest::clean(&s)),
        };
        let release = release(&client, &base, version, source)
            .await
            .unwrap_or_else(|(reason, status)| ReleaseFacts::Unreachable { reason, status });
        Ok(GithubFacts::Checked { repo, release })
    };
    query
        .await
        .unwrap_or_else(|(reason, status)| GithubFacts::Unreachable { reason, status })
}
