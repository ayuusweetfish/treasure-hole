use std::io::Write;

type DynResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> DynResult<bytes::Bytes> {
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

  let body = resp.bytes().await?;
  Ok(body)
}

async fn fetch_json(client: &reqwest::Client, url: String) -> DynResult<(String, serde_json::Value)> {
  let body = fetch_bytes(client, &url).await?;
  let body = String::from_utf8((&body).to_vec())?;
  match serde_json::de::from_str::<serde_json::Value>(&body) {
    Ok(r) => Ok((body, r)),
    _ => panic!("Cannot parse JSON document"),
  }
}

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

async fn fetch_attn_pids(client: &reqwest::Client) -> DynResult<Vec<u64>> {
  let mut pids = vec![];

  for page in 1.. {
    let url = format!("https://tapi.thuhole.com/v3/contents/post/attentions?page={}", page);
    let (_text, attns) = fetch_json(&client, url).await?;

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

async fn fetch_and_save_image(client: &reqwest::Client, url: &str) -> DynResult {
  eprintln!("fetch image {}", url);
  let url = format!("https://i.thuhole.com/{}", url);
  let bytes = fetch_bytes(client, &url).await?;

  let paths = url.split('/').collect::<Vec<_>>();
  let mut f = std::fs::File::create(&paths.last().unwrap())?;
  f.write_all(&bytes)?;

  Ok(())
}

async fn fetch_and_save_posts(
  client: &reqwest::Client,
  pids: &[u64],
  f: &mut std::fs::File,
) -> DynResult<Vec<u64>> {
  let mut images = vec![];
  let mut ref_pids = vec![];

  let re_post_ref = regex::Regex::new(r"#(\d{1,})").unwrap();

  // Fetch each post and write to file
  for pid_chunk in pids.chunks(10) {
    let text_futs = pid_chunk.iter().map(|&pid| {
      eprintln!("{}", pid);
      let url = format!("https://tapi.thuhole.com/v3/contents/post/detail?pid={}", pid);
      fetch_json(&client, url)
    });
    let results = futures::future::try_join_all(text_futs).await?;

    for (post_text, post_json) in results {
      f.write_all(post_text.as_bytes())?;
      f.write_all(",\n".as_bytes())?;

      // Look for image contents
      // Post
      let post = expect_json_type!(post_json.get("post"), Option Object);
      let post_type = expect_json_type!(post.get("type"), Option String);
      if post_type == "image" {
        let image_url = expect_json_type!(post.get("url"), Option String);
        images.push(image_url.clone());
      }
      // Replies
      let replies = expect_json_type!(post_json.get("data"), Option Array);
      for reply in replies {
        let reply_type = expect_json_type!(reply.get("type"), Option String);
        if reply_type == "image" {
          let image_url = expect_json_type!(reply.get("url"), Option String);
          images.push(image_url.clone());
        }
      }

      // Look for post references
      // Post
      let post_text = expect_json_type!(post.get("text"), Option String);
      for cap in re_post_ref.captures_iter(post_text) {
        if let Ok(id) = cap[1].parse::<u64>() {
          ref_pids.push(id);
        }
      }
    }
  }

  let image_futs = images.iter().map(|image_url| fetch_and_save_image(client, &image_url));
  futures::future::try_join_all(image_futs).await?;

  Ok(ref_pids)
}

async fn fetch_attn_all(client: &reqwest::Client) -> DynResult {
  let attn_pids = fetch_attn_pids(&client).await?;
  // println!("{:?}", attn_pids);

  let mut f = std::fs::File::create("data.js")?;
  f.write_all("const posts = [\n".as_bytes())?;

  let ref_pids = fetch_and_save_posts(client, &attn_pids, &mut f).await?;
  
  // Deduplication
  let mut fetched_pids = std::collections::HashSet::new();
  fetched_pids.extend(attn_pids);

  let ref_pids = ref_pids.iter()
    .copied()
    .filter(|pid| !fetched_pids.contains(pid))
    .collect::<Vec<_>>();
  eprintln!("referenced: {:?}", ref_pids);
  fetch_and_save_posts(client, &ref_pids, &mut f).await?;

  f.write_all("];\n".as_bytes())?;

  Ok(())
}

#[tokio::main]
async fn main() -> DynResult {
  let client = reqwest::Client::builder()
    .proxy(reqwest::Proxy::https("http://127.0.0.1:1087")?)
    .build()?;

  // let post = fetch("https://tapi.thuhole.com/v3/contents/post/detail?pid=595301");
  fetch_attn_all(&client).await?;

  Ok(())
}
