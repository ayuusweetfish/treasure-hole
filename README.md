备份树洞关注列表（:construction: 尽快添加图形界面）

## 编译

安装 [Rust](https://www.rust-lang.org/) 编译器，在仓库目录下执行

```
cargo build --release
```

## 运行

```
export TOKEN=<token>                    # 身份令牌
export HTTP_PROXY=http://127.0.0.1:1087 # 代理（可选）
export DIR=save_folder                  # 保存位置，若存在则会被清空
cargo run --release
```

将在指定目录下保存文字、图像内容，以及一个网页 `index.html`。
