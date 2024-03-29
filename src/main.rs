#![windows_subsystem = "windows"]

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

async fn fetch_attn_pids(
  client: &reqwest::Client,
  tx: tokio::sync::mpsc::Sender<(bool, String)>,
) -> DynResult<Vec<u64>> {
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

    tx.send((false, format!("获取收藏列表（第 {} 页，{} 条）", page, pids.len()))).await.unwrap();
    // XXX: debug use
    // if page >= 1 { break; }
  }

  // XXX: debug use
  // return Ok(pids[0..5].to_vec());

  Ok(pids)
}

async fn fetch_and_save_image(
  client: &reqwest::Client,
  url: &str,
  wd: &std::path::Path,
) -> DynResult {
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
  tx: tokio::sync::mpsc::Sender<(bool, String)>,
  pids: &[u64],
  f: &mut std::fs::File,
  wd: &std::path::Path,
) -> DynResult<Vec<u64>> {
  let mut images = vec![];
  let mut ref_pids = vec![];

  let re_post_ref = regex::Regex::new(r"#(\d{1,})").unwrap();

  // Fetch each post and write to file
  for (i, pid_chunk) in pids.chunks(10).enumerate() {
    let text_futs = pid_chunk.iter().map(|&pid| {
      let url = format!("https://tapi.thuhole.com/v3/contents/post/detail?pid={}", pid);
      fetch_json(&client, url)
    });
    tx.send((false, format!("获取帖子内容（{}/{}，起始 #{}）",
      std::cmp::min((i + 1) * 10, pids.len()),
      pids.len(),
      pid_chunk[0],
    ))).await.unwrap();
    let results = futures::future::try_join_all(text_futs).await?;

    for (post_text, post_json) in results {
      // Error?
      let code = expect_json_type!(post_json.get("code"), Option Number).as_i64();
      match code {
        Some(-101) => {
          /*tx.send((false, format!("跳过（信息：{}）",
            expect_json_type!(post_json.get("msg"), Option String)
          ))).await.unwrap();*/
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

  for (i, image_chunk) in images.chunks(10).enumerate() {
    let image_futs = image_chunk.iter().map(
      |image_url| fetch_and_save_image(client, &image_url, wd)
    );
    tx.send((false, format!("保存图片（{}/{}）",
      std::cmp::min((i + 1) * 10, images.len()),
      images.len(),
    ))).await.unwrap();
    futures::future::try_join_all(image_futs).await?;
  }

  Ok(ref_pids)
}

async fn fetch_attn_all(
  client: &reqwest::Client,
  tx: tokio::sync::mpsc::Sender<(bool, String)>,
  wd: &std::path::Path,
  ref_levels: u32,
) -> DynResult {
  let attn_pids = fetch_attn_pids(&client, { let tx = tx.clone(); tx }).await?;
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
  // tx.send((false, format!("database: {:?}", wd_path))).await.unwrap();
  let mut f = std::fs::File::create(wd_path)?;
  f.write_all("const posts = [\n".as_bytes())?;

  let mut img_wd_path = std::path::PathBuf::from(wd);
  img_wd_path.push("images");
  // tx.send((false, format!("images: {:?}", img_wd_path))).await.unwrap();
  std::fs::create_dir(img_wd_path)?;

  // Deduplication
  let mut fetched_pids = std::collections::HashSet::new();

  fetched_pids.extend(attn_pids.iter().copied());
  let mut ref_pids = fetch_and_save_posts(client, { let tx = tx.clone(); tx }, &attn_pids, &mut f, wd).await?;
  
  // Delimiter to denote 'reachable by references'
  f.write_all("'---',\n".as_bytes())?;

  for i in 0..ref_levels {
    tx.send((false, format!("=== 跟随第 {} 层引用 ===", i + 1))).await.unwrap();
    ref_pids = ref_pids.iter()
      .copied()
      .filter(|pid| !fetched_pids.contains(pid))
      .collect::<Vec<_>>();
    fetched_pids.extend(attn_pids.iter().copied());
    // tx.send((false, format!("referenced: {:?}", ref_pids))).await.unwrap();
    ref_pids = fetch_and_save_posts(client, { let tx = tx.clone(); tx }, &ref_pids, &mut f, wd).await?;
  }

  f.write_all("];\n".as_bytes())?;

  // Write HTML
  let mut html_wd_path = std::path::PathBuf::from(wd);
  html_wd_path.push("index.html");
  tx.send((false, format!("保存位置为 {:?}", html_wd_path))).await.unwrap();
  let mut f = std::fs::File::create(html_wd_path)?;
  f.write_all(include_bytes!("../index.html"))?;

  Ok(())
}

async fn fetch_everything(
  token: &str, proxy: &str, target_dir: &std::path::Path,
  ref_levels: u32,
  tx: tokio::sync::mpsc::Sender<(bool, String)>,
) -> DynResult {
  tx.send((false, format!("开始备份，内容将保存在 {:?}\n======", target_dir))).await.unwrap();

  let mut headers = reqwest::header::HeaderMap::new();
  headers.insert("TOKEN", reqwest::header::HeaderValue::from_str(token)?);
  let mut client_builder = reqwest::Client::builder()
    .default_headers(headers);
  if proxy != "" {
    client_builder = client_builder.proxy(reqwest::Proxy::https(proxy)?);
  }
  let client = client_builder.build()?;

  fetch_attn_all(&client, tx, target_dir, ref_levels).await?;

  Ok(())
}

async fn main_gui() -> DynResult {
  let mut ui = iui::UI::init().unwrap();

  use iui::controls::*;

  let mut win = Window::new(&ui, "hole", 360, 540, WindowType::HasMenubar);
  let mut grid = LayoutGrid::new(&ui);
  grid.set_padded(&ui, true);
  win.set_child(&ui, grid.clone());

  let ent_token = Entry::new(&ui);
  let mut ent_reflv = Spinbox::new(&ui, 0, 10);
  let ent_proxy = Entry::new(&ui);
  let controls: [(&str, Control); 3] = [
    ("身份令牌", ent_token.clone().into()),
    ("引用层数", ent_reflv.clone().into()),
    ("网络代理", ent_proxy.clone().into()),
  ];
  ent_reflv.set_value(&ui, 2);

  for (i, (text, control)) in controls.iter().enumerate() {
    let label = Label::new(&ui, text);
    grid.append(&ui, label.clone(), 0, i as i32, 1, 1,
      GridExpand::Neither, GridAlignment::Fill, GridAlignment::Fill);
    grid.append(&ui, control.clone(), 1, i as i32, 1, 1,
      GridExpand::Horizontal, GridAlignment::Fill, GridAlignment::Fill);
  }

  let ent_reflv_text = Entry::new(&ui);
  grid.append(&ui, ent_reflv_text.clone(), 1, 1, 1, 1,
    GridExpand::Horizontal, GridAlignment::Fill, GridAlignment::Fill);
  ui.set_enabled(ent_reflv_text.clone(), false);
  ui.set_shown(ent_reflv_text.clone(), false);

  let mut btn_go = Button::new(&ui, "开始");
  grid.append(&ui, btn_go.clone(), 0, 4, 2, 1,
    GridExpand::Horizontal, GridAlignment::Fill, GridAlignment::Fill);

  let log_disp = MultilineEntry::new(&ui);
  ui.set_enabled(log_disp.clone(), false);
  ui.set_shown(log_disp.clone(), false);
  grid.append(&ui, log_disp.clone(), 0, 5, 2, 1,
    GridExpand::Both, GridAlignment::Fill, GridAlignment::Fill);

  let handle = tokio::runtime::Handle::current();
  let (tx, mut rx) = tokio::sync::mpsc::channel(10);
  let mut logs = vec![];

  btn_go.on_clicked(&ui, {
    let mut ui = ui.clone();
    let controls = controls.clone();
    let ent_reflv = ent_reflv.clone();
    let mut ent_reflv_text = ent_reflv_text.clone();
    let btn_go = btn_go.clone();
    let log_disp = log_disp.clone();
    move |_| {
      let token = ent_token.value(&ui);
      let proxy = ent_proxy.value(&ui);
      let reflv = ent_reflv.value(&ui) as u32;
      // Directory of executable
      let mut wd = std::env::current_exe().unwrap();
      wd.pop();
      let time = chrono::offset::Local::now().format("%Y%m%d-%H%M");
      wd.push(&format!("{}-{}", time, token));

      // Disable controls
      for (_, control) in &controls {
        ui.set_enabled(control.clone(), false);
      }
      ent_reflv_text.set_value(&ui, &ent_reflv.value(&ui).to_string());
      ui.set_shown(ent_reflv.clone(), false);
      ui.set_shown(ent_reflv_text.clone(), true);
      ui.set_enabled(btn_go.clone(), false);
      ui.set_shown(log_disp.clone(), true);

      // Spawn thread
      let tx = tx.clone();
      handle.spawn(async move {
        let msg = match fetch_everything(
          &token,
          &proxy,
          &wd,
          reflv,
          { let tx = tx.clone(); tx },
        ).await {
          Err(e) => format!("出现意外问题，请在汇报时附上以下信息：\n{}\n======", e),
          _ => "完成".to_string(),
        };
        tx.send((true, msg)).await.unwrap();
      });
    }
  });

  win.show(&ui);

  let mut event_loop = ui.event_loop();
  event_loop.on_tick(&ui, {
    let mut ui = ui.clone();
    let controls = controls.clone();
    let ent_reflv = ent_reflv.clone();
    let ent_reflv_text = ent_reflv_text.clone();
    let mut log_disp = log_disp.clone();
    let logs = &mut logs;
    move || {
      if let Ok((term, text)) = rx.try_recv() {
        // Add to log
        logs.insert(0, text);
        if logs.len() >= 500 { logs.pop(); }
        log_disp.set_value(&ui, &logs.join("\n"));

        if term {
          // Enable controls
          for (_, control) in &controls {
            ui.set_enabled(control.clone(), true);
          }
          ui.set_shown(ent_reflv.clone(), true);
          ui.set_shown(ent_reflv_text.clone(), false);
          ui.set_enabled(btn_go.clone(), true);
        }
      }
    }
  });
  event_loop.run_delay(&ui, 10);

  Ok(())
}

#[tokio::main]
async fn main() -> DynResult {
  let args = std::env::args().collect::<Vec<_>>();
  if args.len() >= 4 {
    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    let join_handle = tokio::runtime::Handle::current().spawn(async move {
      loop {
        if let Some((term, text)) = rx.recv().await {
          eprintln!("{}", text);
          std::io::stderr().flush().unwrap();
          if term { break; }
        } else {
          break;
        }
      }
    });
    let result = fetch_everything(
      &args[1],
      if args.len() >= 5 { &args[4] } else { "" },
      std::path::Path::new(&args[3]),
      args[2].parse()?,
      tx.clone(),
    ).await;
    if let Err(e) = result {
      tx.clone().send((true, format!("出现意外问题：\n{}\n======", e))).await.unwrap();
    } else {
      tx.clone().send((true, "完成".to_string())).await.unwrap();
    }
    join_handle.await?;
    std::io::stderr().flush().unwrap();
  } else {
    main_gui().await?;
  }
  Ok(())
}
