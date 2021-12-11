type DynResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[tokio::main]
async fn main() -> DynResult {
  let client = reqwest::Client::builder()
    .proxy(reqwest::Proxy::https("http://127.0.0.1:1087")?)
    .build()?;
  let body =
    client.get("https://tapi.thuhole.com/v3/contents/post/detail?pid=489292")
      .header("TOKEN", include_str!("token.txt").trim())
      .send().await?
      .text().await?;
  println!("{:?}", body);

  Ok(())
}
