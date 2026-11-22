"""Run both real startup readers against the other writer: python tests/startup_compat.py."""
import io
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import zipfile

ROOT = Path(__file__).resolve().parents[1]
BASE = "390cb492"
# This harness is compiled against the baseline library, not a reimplementation
# of its reader. Inspect the slot before invoking its actual wait_ready path.
LEGACY_TEST = r'''
mod common;
use aproxy::{config::Overrides, spawn::{spawn_from_spec, SpawnError, READY_TIMEOUT}, spec};
#[tokio::test(flavor="current_thread")]
async fn old_reader_preserves_the_new_writers_english_log() {
    let h=common::TestHome::new();
    let writer=std::path::PathBuf::from(std::env::var_os("APROXY_STARTUP_WRITER").unwrap());
    let missing=h.path().join("absent.toml");
    let spec=spec::capture(Some(&missing),Overrides::default(),&h.path().join("instance.log"));
    let slot=spec::write_spec_file(&h.path().join("run"),&spec).unwrap();
    let out=h.cmd_with(&writer).arg("__serve").arg("--home").arg(h.path()).arg("--spec").arg(&slot).output().unwrap();
    assert_eq!(out.status.code(),Some(1),"{out:?}");
    let raw=std::fs::read(slot).unwrap();
    let payload:serde_json::Value=serde_json::from_slice(&raw).unwrap();
    assert_eq!(payload["render"]["id"],"missing");
    let err=spawn_from_spec(h.path(),&writer,&spec,READY_TIMEOUT).await.unwrap_err();
    let SpawnError::Exited{format:None,startup_log,..}=err else {panic!("{err:?}")};
    assert_eq!(startup_log.lines.len(),1);
    let line=&startup_log.lines[0];
    assert!(line.ends_with(&format!("Configuration file not found: {}",missing.display())));
    assert!(startup_log.excerpt().contains(line));
}
'''


def main():
    with TemporaryDirectory(prefix="aproxy-startup-") as scratch:
        tmp = Path(scratch)
        old = tmp / "baseline"
        archive = subprocess.check_output(["git", "archive", "--format=zip", BASE], cwd=ROOT)
        with zipfile.ZipFile(io.BytesIO(archive)) as z:
            z.extractall(old)
        (old / "tests/startup_compat.rs").write_text(LEGACY_TEST, encoding="utf-8")
        flags = ["-p", "aproxy", "--no-default-features", "--locked", "--offline"]
        target = Path(os.environ.get("CARGO_TARGET_DIR", tmp / "current-target")).resolve()
        targets = [target, target.with_name(target.name + "-startup-baseline")]
        env = dict(os.environ, APROXY_LANG="en")
        for source, build in zip([ROOT, old], targets):
            subprocess.run(["cargo", "build", *flags, "--bin", "aproxy"], cwd=source,
                           env=dict(env, CARGO_TARGET_DIR=str(build)), check=True)
        exe = "aproxy.exe" if os.name == "nt" else "aproxy"
        for source, build, peer, var in [(ROOT, targets[0], targets[1], "APROXY_STARTUP_LEGACY_WRITER"),
                                         (old, targets[1], targets[0], "APROXY_STARTUP_WRITER")]:
            subprocess.run(["cargo", "test", *flags, "--test", "startup_compat"], cwd=source,
                           env=dict(env, CARGO_TARGET_DIR=str(build), **{var: str(peer / "debug" / exe)}), check=True)


if __name__ == "__main__":
    main()
