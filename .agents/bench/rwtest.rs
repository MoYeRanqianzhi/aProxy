use std::time::Duration;

#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .post("http://127.0.0.1:59996/v1/chat")
        .body("{}")
        .send()
        .await
        .unwrap();
    println!("status: {}", resp.status());
    let mut resp = resp;
    let mut total = 0usize;
    loop {
        match resp.chunk().await {
            Ok(Some(c)) => {
                total += c.len();
                print!("chunk {}B ", c.len());
            }
            Ok(None) => {
                println!("\nEOF, total={total}");
                break;
            }
            Err(e) => {
                println!("\nERR: {e}");
                break;
            }
        }
    }
    let _ = Duration::ZERO;
}
