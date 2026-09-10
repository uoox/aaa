//! v1.35 产物里的 Markdown：项目产出的 `.md` 清单 + 只读地读一个文件。
//!
//! v1.30 的目录 explorer（会话的第三种看法「浏览」）2026-09-10 删掉了，换成
//! **项目自己产出的 Markdown 直接排进详情栏的「产物」一节**——手机上真正会去翻的
//! 只有报告和笔记，而不是一个 `.git/objects` 也点得进去的文件管理器。
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

/// 一个项目最多列这么多份 Markdown（够多了；再多就不是「产物」而是一个仓库）。
pub const MAX_DOCS: usize = 200;
/// 往下找几层。报告放在根上或 `docs/`、`reports/` 底下，四层足够，
/// 再深就该由 agent 告诉你它写在哪，而不是这里去爬整棵树。
pub const MAX_DOC_DEPTH: usize = 4;
/// 扫描时最多看这么多个目录项：`node_modules` 已经被跳掉了，这一条是兜底，
/// 免得一个病态的目录树把这次请求拖死。
const MAX_DOC_VISITS: usize = 20_000;
/// 一次最多读这么多字节的正文，超出的截断（`truncated:true`）。
pub const MAX_BYTES: usize = 512 * 1024;

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
        // html 单列一类：客户端要么渲染（Android 有 WebView），要么交给系统默认程序
        // （mac 上文件就在本机，`open` 一下就是浏览器）。正文照旧当文本给，看源码也行
        "html" | "htm" => "html",
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

/// 一份项目产出的 Markdown（详情栏「产物」一节里的一行）。
#[derive(Serialize, PartialEq, Debug)]
pub struct Doc {
    pub name: String,
    pub path: String,
    /// 相对项目根的位置（`docs/api.md`）。就在根上时等于 `name`
    pub rel: String,
    pub size: u64,
    pub mtime: f64,
}

/// 不进「产物」的目录：点开头的（`.git`、`.venv`…）、装依赖和放构建产物的，
/// 还有 `_inbox`——那是**传进来的**文件，详情栏里自有「已上传」一节。
fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "build" | "dist" | "out" | "vendor" | "venv"
                | "__pycache__" | "_inbox" | "Pods"
        )
}

/// 项目根底下的 Markdown，**最近改的在前**。广度优先，一层层往下，到
/// [`MAX_DOC_DEPTH`] 为止；`project` 必须已经是 canonicalize 过的路径。
///
/// 为什么是「扫目录」而不是「从 transcript 里认」：报告常常是**上一次**会话写的，
/// 而 transcript 只认得这一条会话干过的事。项目里有哪些 Markdown 是项目的属性，
/// 不是某一条对话的属性。
pub fn find_docs(project: &Path) -> Vec<Doc> {
    let mut out: Vec<Doc> = Vec::new();
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> =
        std::collections::VecDeque::from([(project.to_path_buf(), 0)]);
    let mut visits = 0usize;
    while let Some((dir, depth)) = queue.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            visits += 1;
            if visits > MAX_DOC_VISITS {
                queue.clear();
                break;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            // **软链接一律不跟**（`file_type` 是这一项自己的类型，不解链接）：指回上层的
            // 链接会让广度优先在同一批文件上转圈、每层再列一遍；指到项目根外的链接会列出
            // 一份点开就 404 的文件（读那一步的守卫会 canonicalize 后把它挡在外面）
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            // 元数据取不到（刚被删掉）就当它不存在
            let Ok(md) = e.metadata() else { continue };
            if ft.is_dir() {
                if depth + 1 < MAX_DOC_DEPTH && !skip_dir(&name) {
                    queue.push_back((e.path(), depth + 1));
                }
                continue;
            }
            if kind_of(&name) != "markdown" {
                continue;
            }
            let path = e.path();
            let rel = path
                .strip_prefix(project)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| name.clone());
            out.push(Doc {
                name,
                path: path.to_string_lossy().into_owned(),
                rel,
                size: md.len(),
                mtime: crate::stores::mtime_f(&md),
            });
        }
    }
    // 最近改的在前；同一刻按位置稳住，免得两次请求给出不同的顺序
    out.sort_by(|a, b| {
        b.mtime
            .partial_cmp(&a.mtime)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.rel.cmp(&b.rel))
    });
    out.truncate(MAX_DOCS);
    out
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
/// 都只报大小（`kind:"binary"`，`text` 空）。html 与 markdown 一样按文本读出来。
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
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 产物里的 Markdown：只收 `.md` 一类，最近改的在前，装依赖 / 放上传的目录不进去。
    #[test]
    fn docs_are_markdown_only_newest_first() {
        let root = tmp("docs");
        for d in ["docs", "node_modules/pkg", ".git", "_inbox", "a/b/c/d"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let write = |rel: &str, secs: u64| {
            let p = root.join(rel);
            std::fs::write(&p, b"# hi").unwrap();
            let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_780_000_000 + secs);
            filetime(&p, t);
        };
        write("README.md", 10);
        write("docs/api.md", 30);
        write("app.rs", 40);
        write("node_modules/pkg/readme.md", 50);
        write(".git/notes.md", 50);
        write("_inbox/传进来的.md", 50);
        write("a/b/c/d/deep.md", 60);
        // 指回上层的软链接：跟着它走会在同一批文件上转圈，每层再列一遍
        std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
        std::os::unix::fs::symlink(root.join("README.md"), root.join("alias.md")).unwrap();

        let docs = find_docs(&root);
        let rels: Vec<&str> = docs.iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(rels, vec!["docs/api.md", "README.md"], "只剩这两份，新的在前；软链接一个都不跟");
        assert_eq!(docs[1].name, "README.md");
        assert!(docs[0].path.ends_with("docs/api.md"), "path 是绝对路径，读的时候直接用");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 改一个文件的 mtime（测试要的是确定的顺序，不是「写得快不快」）
    fn filetime(p: &Path, t: std::time::SystemTime) {
        let secs = t.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as libc::time_t;
        let times = [
            libc::timeval { tv_sec: secs, tv_usec: 0 },
            libc::timeval { tv_sec: secs, tv_usec: 0 },
        ];
        let c = std::ffi::CString::new(p.to_string_lossy().as_bytes()).unwrap();
        unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) };
    }

    /// 读：markdown 出正文；二进制只报大小；超长截断但不把好的那半截扔掉。
    #[test]
    fn read_handles_markdown_binary_and_truncation() {
        let root = tmp("read");
        std::fs::write(root.join("page.html"), b"<h1>hi</h1>").unwrap();
        let html = read_file(&root.join("page.html")).unwrap();
        assert_eq!((html.kind, html.text.as_str()), ("html", "<h1>hi</h1>"), "html 是单独一类，正文照给");

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
