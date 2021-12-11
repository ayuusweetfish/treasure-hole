use std::io::Write;

#[derive(Debug)]
pub struct StringError {
  s: String,
}

impl StringError {
  pub fn new(s: String) -> Self {
    Self { s }
  }
}

impl std::convert::From<&str> for StringError {
  fn from(s: &str) -> Self {
    Self { s: s.to_string() }
  }
}
impl std::convert::From<String> for StringError {
  fn from(s: String) -> Self {
    Self { s }
  }
}

impl std::fmt::Display for StringError {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(f, "{}", self.s)
  }
}
impl std::error::Error for StringError {}

type DynResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

async fn fetch_bytes(client: &reqwest::Client, url: &str) -> DynResult<bytes::Bytes> {
  let resp = client.get(url).send().await?;
  let status = resp.status();
  if status.as_u16() != 200 {
    return Err(Box::new(StringError::from(format!("HTTP Status: {} {}",
      status.as_u16(),
      status.canonical_reason().unwrap_or("Unknown reason")))));
  }

  let body = resp.bytes().await?;
  Ok(body)
}

async fn fetch_json(client: &reqwest::Client, url: String) -> DynResult<(String, serde_json::Value)> {
  let body = fetch_bytes(client, &url).await?;
  let body = String::from_utf8((&body).to_vec())?;
  match serde_json::de::from_str::<serde_json::Value>(&body) {
    Ok(r) => Ok((body, r)),
    _ => Err(Box::new(StringError::from("Cannot parse JSON document"))),
  }
}

macro_rules! expect_json_type {
  // Optional types
  ($value:expr, Option $variant:tt) => {
    match $value {
      Some(serde_json::Value::$variant(x)) => x,
      _ => return Err(Box::new(StringError::from(
        format!("Incorrect JSON format (line {})", line!())))),
    }
  };
  // Direct types
  ($value:expr, $variant:tt) => {
    match $value {
      serde_json::Value::$variant(x) => x,
      _ => return Err(Box::new(StringError::from(
        format!("Incorrect JSON format (line {})", line!())))),
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
    let code = expect_json_type!(attns.get("code"), Option Number).as_i64();
    match code {
      Some(code) if code != 0 => {
        return Err(Box::new(StringError::from(format!(
          "Incorrect code {}; message {}", code,
          expect_json_type!(attns.get("msg"), Option String)
        ))));
      },
      None => return Err(Box::new(StringError::from("Incorrect JSON format"))),
      _ => {},
    }

    let posts = expect_json_type!(attns.get("data"), Option Array);
    if posts.len() == 0 { break; }

    for post in posts {
      let pid = match expect_json_type!(post.get("pid"), Option Number).as_u64() {
        Some(x) => x,
        None => return Err(Box::new(StringError::from("Incorrect JSON format"))),
      };
      pids.push(pid);
    }

    eprintln!("page {}; count {}", page, pids.len());
    // XXX: debug use
    // if page >= 1 { break; }
  }

  Ok(pids)
}

async fn fetch_and_save_image(
  client: &reqwest::Client,
  url: &str,
  wd: &std::path::Path,
) -> DynResult {
  eprintln!("fetch image {}", url);
  let url = format!("https://i.thuhole.com/{}", url);
  let bytes = fetch_bytes(client, &url).await?;

  let paths = url.split('/').collect::<Vec<_>>();
  let mut wd_path = std::path::PathBuf::from(wd);
  wd_path.push("images");
  wd_path.push(paths.last().unwrap());
  let mut f = std::fs::File::create(&wd_path)?;
  f.write_all(&bytes)?;

  Ok(())
}

async fn fetch_and_save_posts(
  client: &reqwest::Client,
  pids: &[u64],
  f: &mut std::fs::File,
  wd: &std::path::Path,
) -> DynResult<Vec<u64>> {
  let mut images = vec![];
  let mut ref_pids = vec![];

  let re_post_ref = regex::Regex::new(r"#(\d{1,})").unwrap();

  // Fetch each post and write to file
  for pid_chunk in pids.chunks(10) {
    let text_futs = pid_chunk.iter().map(|&pid| {
      eprintln!("fetch post {}", pid);
      let url = format!("https://tapi.thuhole.com/v3/contents/post/detail?pid={}", pid);
      fetch_json(&client, url)
    });
    let results = futures::future::try_join_all(text_futs).await?;

    for (post_text, post_json) in results {
      // Error?
      let code = expect_json_type!(post_json.get("code"), Option Number).as_i64();
      match code {
        Some(-101) => {
          eprintln!("Skipping (message: {})",
            expect_json_type!(post_json.get("msg"), Option String)
          );
          continue;
        },
        Some(code) if code != 0 => {
          return Err(Box::new(StringError::from(format!(
            "Incorrect code {}; message {}", code,
            expect_json_type!(post_json.get("msg"), Option String)
          ))));
        },
        None => return Err(Box::new(StringError::from("Incorrect JSON format"))),
        _ => {},
      }

      f.write_all(post_text.as_bytes())?;
      f.write_all(",\n".as_bytes())?;
      f.flush()?;

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

  for image_chunk in images.chunks(10) {
    let image_futs = image_chunk.iter().map(
      |image_url| fetch_and_save_image(client, &image_url, wd)
    );
    futures::future::try_join_all(image_futs).await?;
  }

  Ok(ref_pids)
}

async fn fetch_attn_all(client: &reqwest::Client, wd: &std::path::Path) -> DynResult {
  let attn_pids = fetch_attn_pids(&client).await?;
  // println!("{:?}", attn_pids);

  match std::fs::remove_dir_all(wd) {
    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
      return Err(Box::new(StringError::from("Cannot remove existing contents")));
    },
    _ => {},
  }
  std::fs::create_dir(wd)?;

  let mut wd_path = std::path::PathBuf::from(wd);
  wd_path.push("data.js");
  eprintln!("database: {:?}", wd_path);
  let mut f = std::fs::File::create(wd_path)?;
  f.write_all("const posts = [\n".as_bytes())?;

  let mut img_wd_path = std::path::PathBuf::from(wd);
  img_wd_path.push("images");
  eprintln!("images: {:?}", img_wd_path);
  std::fs::create_dir(img_wd_path)?;

  // Deduplication
  let mut fetched_pids = std::collections::HashSet::new();

  fetched_pids.extend(attn_pids.iter().copied());
  let mut ref_pids = fetch_and_save_posts(client, &attn_pids, &mut f, wd).await?;
  
  // Delimiter to denote 'reachable by references'
  f.write_all("'---',\n".as_bytes())?;

  for _ in 0..2 {
    ref_pids = ref_pids.iter()
      .copied()
      .filter(|pid| !fetched_pids.contains(pid))
      .collect::<Vec<_>>();
    fetched_pids.extend(attn_pids.iter().copied());
    eprintln!("referenced: {:?}", ref_pids);
    ref_pids = fetch_and_save_posts(client, &ref_pids, &mut f, wd).await?;
  }

  f.write_all("];\n".as_bytes())?;

  // Write HTML
  let mut html_wd_path = std::path::PathBuf::from(wd);
  html_wd_path.push("index.html");
  eprintln!("page: {:?}", html_wd_path);
  let mut f = std::fs::File::create(html_wd_path)?;
  f.write_all(include_bytes!("../index.html"))?;

  Ok(())
}

#[tokio::main]
async fn main() -> DynResult {
  let token = std::env::var("TOKEN")?;
  let proxy = std::env::var("HTTP_PROXY")?;
  let target_dir = std::env::var("DIR")?;

  let mut headers = reqwest::header::HeaderMap::new();
  headers.insert("TOKEN", reqwest::header::HeaderValue::from_str(&token)?);
  let mut client_builder = reqwest::Client::builder()
    .default_headers(headers);
  if proxy != "" {
    client_builder = client_builder.proxy(reqwest::Proxy::https(&proxy)?);
  }
  let client = client_builder.build()?;

  let path = std::path::Path::new(&target_dir);
  fetch_attn_all(&client, path).await?;

  Ok(())
}
