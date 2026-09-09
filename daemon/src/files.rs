//! v1.30 目录浏览：项目根底下的只读文件浏览器（客户端第三种视图，见 PROTOCOL「浏览」）。
//!
//! **只读**：没有写、改名、删除。手机上要改文件就跟 agent 说，那是 agent 的活。
//!
//! 两条硬约束，两端都不必再各自判一遍：
//! - **不出项目根**：路径先 canonicalize（软链接、`..` 都在这一步解掉），再要求它
//!   落在同样 canonicalize 过的项目根底下。软链接指到根外的那一刻就被挡住，
//!   `starts_with` 是按路径段比的，`/Volumes/SSD/project-old` 不会被当成
//!   `/Volumes/SSD/project` 的孩子。
//! - **不读二进制**：文件内容按 UTF-8 解，解不动就只报大小（`kind:"binary"`），
//!   不往客户端灌一兆乱码。

use std::path::{Path, PathBuf};

use serde::Serialize;

/// 一次最多列这么多条：`.git/objects` 那种目录几万个文件，全发过去手机先卡死。
pub const MAX_ENTRIES: usize = 2000;
/// 一次最多读这么多字节的正文，超出的截断（`truncated:true`）。
pub const MAX_BYTES: usize = 512 * 1024;

#[derive(Serialize, PartialEq, Debug)]
pub struct Entry {
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub size: u64,
    pub mtime: f64,
    /// markdown | text | binary（目录为空串）。客户端据此决定点开是渲染还是等宽显示。
    pub kind: &'static str,
}

/// 后缀 → 这个文件点开该怎么显示。**只看后缀**：内容嗅探要把文件读进来，
/// 而列目录时读几千个文件的头几个字节比列目录本身还贵。真读的时候
/// （[`read_file`]）UTF-8 解不动会把它改判成 `binary`。
pub fn kind_of(name: &str) -> &'static str {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "md" | "markdown" | "mdx" => "markdown",
        // 二进制的常见几类：点开只报大小，不灌乱码
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "heic" | "ico" | "pdf" | "zip" | "gz"
        | "tar" | "bz2" | "xz" | "7z" | "rar" | "mp3" | "mp4" | "mov" | "wav" | "m4a"
        | "avi" | "mkv" | "woff" | "woff2" | "ttf" | "otf" | "so" | "dylib" | "a" | "o"
        | "class" | "jar" | "apk" | "dmg" | "bin" | "wasm" | "db" | "sqlite" | "pb" => "binary",
        _ => "text",
    }
}

/// 把客户端给的路径解成一个「确实在项目根底下」的真实路径。
/// `root` 必须已经是 canonicalize 过的（config 装载时就做了）。
pub fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let p = PathBuf::from(path);
    if !p.is_absolute() {
        return None;
    }
    let real = std::fs::canonicalize(&p).ok()?;
    (real == root || real.starts_with(root)).then_some(real)
}

/// 上一级——**到项目根为止**。根自己没有上一级（客户端据此不画「..」）。
pub fn parent_of(root: &Path, dir: &Path) -> Option<String> {
    if dir == root {
        return None;
    }
    dir.parent()
        .filter(|p| p.starts_with(root) || *p == root)
        .map(|p| p.to_string_lossy().into_owned())
}

/// 目录列表：目录在前，其次按名字（大小写无关）。点开头的不特殊对待——
/// `.gitignore`、`.aaa-agents` 正是要看的东西。
pub fn list_dir(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let Ok(e) = e else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        // 元数据取不到（断掉的软链接、刚被删掉）就当它不存在，不为一条坏项目废掉整个列表
        let Ok(md) = e.metadata() else { continue };
        let dir_flag = md.is_dir();
        out.push(Entry {
            kind: if dir_flag { "" } else { kind_of(&name) },
            path: e.path().to_string_lossy().into_owned(),
            name,
            dir: dir_flag,
            size: if dir_flag { 0 } else { md.len() },
            mtime: crate::stores::mtime_f(&md),
        });
        if out.len() >= MAX_ENTRIES {
            break;
        }
    }
    out.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(out)
}

#[derive(Serialize)]
pub struct FileBody {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime: f64,
    pub kind: &'static str,
    pub text: String,
    pub truncated: bool,
}

/// 读一个文件的正文。后缀说是二进制的、或者读出来不是合法 UTF-8 的，
/// 都只报大小（`kind:"binary"`，`text` 空）。
pub fn read_file(path: &Path) -> std::io::Result<FileBody> {
    use std::io::Read;
    let md = std::fs::metadata(path)?;
    if md.is_dir() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "是一个目录"));
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let mut body = FileBody {
        path: path.to_string_lossy().into_owned(),
        kind: kind_of(&name),
        name,
        size: md.len(),
        mtime: crate::stores::mtime_f(&md),
        text: String::new(),
        truncated: false,
    };
    if body.kind == "binary" {
        return Ok(body);
    }
    let mut buf = Vec::new();
    std::fs::File::open(path)?
        .take(MAX_BYTES as u64)
        .read_to_end(&mut buf)?;
    body.truncated = md.len() > buf.len() as u64;
    match String::from_utf8(buf) {
        Ok(text) => body.text = text,
        Err(e) => {
            // 截断正好切在一个多字节字符中间 ≠ 这是二进制文件：切口之前那一段仍是好的
            let good = e.utf8_error().valid_up_to();
            if body.truncated && good > 0 {
                let mut bytes = e.into_bytes();
                bytes.truncate(good);
                body.text = String::from_utf8(bytes).unwrap_or_default();
            } else {
                body.kind = "binary";
            }
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("aaa-files-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::canonicalize(&p).unwrap()
    }

    /// 出不了项目根：`..` 与「同前缀的另一个目录」都得挡住。
    #[test]
    fn resolve_stays_inside_the_root() {
        let base = tmp("resolve");
        let root = base.join("project");
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::create_dir_all(base.join("project-old")).unwrap();
        assert_eq!(resolve(&root, root.to_str().unwrap()), Some(root.clone()));
        assert_eq!(resolve(&root, root.join("a/b").to_str().unwrap()), Some(root.join("a/b")));
        assert_eq!(resolve(&root, root.join("a/../a/b").to_str().unwrap()), Some(root.join("a/b")));
        assert_eq!(resolve(&root, base.join("project-old").to_str().unwrap()), None, "同前缀不是孩子");
        assert_eq!(resolve(&root, base.to_str().unwrap()), None, "上一级出根了");
        assert_eq!(resolve(&root, "relative/path"), None, "只收绝对路径");
        // 指到根外的软链接：canonicalize 解掉之后就露馅了
        let outside = base.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert_eq!(resolve(&root, root.join("link").to_str().unwrap()), None, "软链接不是后门");
        assert_eq!(parent_of(&root, &root), None, "根没有上一级");
        assert_eq!(parent_of(&root, &root.join("a/b")), Some(root.join("a").to_string_lossy().into_owned()));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 列表：目录在前、名字大小写无关排序；点开头的照列。
    #[test]
    fn list_puts_directories_first() {
        let root = tmp("list");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("README.md"), b"# hi").unwrap();
        std::fs::write(root.join("app.rs"), b"fn main(){}").unwrap();
        std::fs::write(root.join(".gitignore"), b"target").unwrap();
        let e = list_dir(&root).unwrap();
        let names: Vec<&str> = e.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["src", ".gitignore", "app.rs", "README.md"]);
        assert_eq!(e[0].kind, "", "目录不谈 kind");
        assert_eq!(e[2].kind, "text");
        assert_eq!(e[3].kind, "markdown");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 读：markdown 出正文；二进制只报大小；超长截断但不把好的那半截扔掉。
    #[test]
    fn read_handles_markdown_binary_and_truncation() {
        let root = tmp("read");
        std::fs::write(root.join("a.md"), "# 标题\n正文".as_bytes()).unwrap();
        let md = read_file(&root.join("a.md")).unwrap();
        assert_eq!((md.kind, md.text.as_str(), md.truncated), ("markdown", "# 标题\n正文", false));

        std::fs::write(root.join("x.png"), [0x89u8, 0x50, 0x4e, 0x47]).unwrap();
        let png = read_file(&root.join("x.png")).unwrap();
        assert_eq!((png.kind, png.size), ("binary", 4));
        assert!(png.text.is_empty(), "二进制不灌乱码");

        // 后缀说是文本、内容却不是 UTF-8 → 改判 binary
        std::fs::write(root.join("y.txt"), [0xffu8, 0xfe, 0x00]).unwrap();
        assert_eq!(read_file(&root.join("y.txt")).unwrap().kind, "binary");

        let long = "中".repeat(MAX_BYTES); // 每个 3 字节，必然截断，且切口落在字符中间
        std::fs::write(root.join("big.txt"), &long).unwrap();
        let big = read_file(&root.join("big.txt")).unwrap();
        assert!(big.truncated);
        assert_eq!(big.kind, "text", "截断不等于二进制");
        assert!(big.text.starts_with('中') && !big.text.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
