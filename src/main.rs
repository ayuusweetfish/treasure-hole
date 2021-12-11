use std::io::Write;

type DynResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn fetch_text(client: &reqwest::Client, url: &str) -> DynResult<String> {
  let resp =
    client.get(url)
      .header("TOKEN", include_str!("token.txt").trim())
      .send().await?;
  let status = resp.status();
  if status.as_u16() != 200 {
    panic!("HTTP Status: {} {}",
      status.as_u16(),
      status.canonical_reason().unwrap_or("Unknown reason"));
  }

  let body = resp.text().await?;
  Ok(body)
}

async fn fetch_json(client: &reqwest::Client, url: &str) -> DynResult<serde_json::Value> {
  let body = fetch_text(client, url).await?;
  serde_json::de::from_str::<serde_json::Value>(&body)
    .or_else(|_| panic!("Cannot parse JSON document"))
}

async fn fetch_attn_pids(client: &reqwest::Client) -> DynResult<Vec<u64>> {
  let mut pids = vec![];

  for page in 1.. {
    let url = format!("https://tapi.thuhole.com/v3/contents/post/attentions?page={}", page);
    let attns = fetch_json(&client, &url).await?;

    macro_rules! expect_json_type {
      // Optional types
      ($value:expr, Option $variant:tt) => {
        match $value {
          Some(serde_json::Value::$variant(x)) => x,
          _ => panic!("Incorrect JSON format"),
        }
      };
      // Direct types
      ($value:expr, $variant:tt) => {
        match $value {
          serde_json::Value::$variant(x) => x,
          _ => panic!("Incorrect JSON format"),
        }
      };
    }

    let attns = expect_json_type!(attns, Object);
    // println!("{:?}", attns);

    // Return code
    let code = expect_json_type!(attns.get("code"), Option Number)
      .as_i64().unwrap_or_else(|| panic!("Incorrect JSON format"));
    if code != 0 {
      panic!("Incorrect code {}; message {}", code,
        expect_json_type!(attns.get("msg"), Option String));
    }

    let posts = expect_json_type!(attns.get("data"), Option Array);
    if posts.len() == 0 { break; }

    for post in posts {
      let pid = match expect_json_type!(post.get("pid"), Option Number).as_u64() {
        Some(x) => x,
        None => panic!("Incorrect JSON format"),
      };
      pids.push(pid);
    }

    eprintln!("page {}; count {}", page, pids.len());
    // XXX: debug use
    if page >= 3 { break; }
  }

  Ok(pids)
}

#[tokio::main]
async fn main() -> DynResult {
  let client = reqwest::Client::builder()
    .proxy(reqwest::Proxy::https("http://127.0.0.1:1087")?)
    .build()?;

  // let post = fetch("https://tapi.thuhole.com/v3/contents/post/detail?pid=595301");

  let attn_pids = fetch_attn_pids(&client).await?;
  // println!("{:?}", attn_pids);

  let mut f = std::fs::File::create("data.js")?;
  f.write_all("const posts = [\n".as_bytes())?;
  // Fetch each post and write to file
  for pid in attn_pids {
    eprintln!("{}", pid);
    let url = format!("https://tapi.thuhole.com/v3/contents/post/detail?pid={}", pid);
    let post = fetch_text(&client, &url).await?;
    f.write_all(post.as_bytes())?;
    f.write_all(",\n".as_bytes())?;
  }
  f.write_all("];\n".as_bytes())?;

  Ok(())
}
