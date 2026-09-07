//! Markdown → 块模型。消息流里 assistant 的 `text`（最终回复与过程中的助手文本）按
//! CommonMark（+ GFM 表格 / 删除线）解析；user / tool / thinking 仍是纯文本，不经过这里。
//!
//! 这是纯数据层：不出现任何 gpui 类型，渲染由 ui/messages_view.rs 消费 [`Block`] 完成，
//! 所以可以直接单元测试。约定：
//! - 软换行 → 空格；硬换行 → 段落内 '\n'
//! - 行内 HTML / HTML 块 → 原文字面文本
//! - 图片 → alt 文本作为链接 span（alt 为空时用 URL 兜底，保证可见可点）
//! - 未知或未启用的构造一律退化成普通文本；任何输入都不 panic

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

/// 行内片段：一段样式一致的文本。相邻同样式片段在解析时已合并。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub strike: bool,
    pub link: Option<String>,
}

impl Span {
    fn same_style(&self, o: &Span) -> bool {
        self.bold == o.bold
            && self.italic == o.italic
            && self.code == o.code
            && self.strike == o.strike
            && self.link == o.link
    }
}

/// 块级元素
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading {
        level: u8,
        spans: Vec<Span>,
    },
    Paragraph(Vec<Span>),
    Code {
        lang: String,
        text: String,
    },
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Rule,
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
    },
}

/// 解析 Markdown 为块列表。空输入 / 只有空白 → 空 Vec（调用方回落纯文本）。
pub fn parse(md: &str) -> Vec<Block> {
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut b = Builder::new();
    for ev in Parser::new_ext(md, opts) {
        b.event(ev);
    }
    b.finish()
}

/// 块列表 → 纯文本（测试 / 回落用；bin crate 里非测试路径暂无调用方）
#[cfg_attr(not(test), allow(dead_code))]
pub fn plain(blocks: &[Block]) -> String {
    blocks.iter().map(block_plain).collect::<Vec<_>>().join("\n\n")
}

/// 表格 → 等宽对齐文本：列宽取该列最宽单元格（CJK 记 2 格），表头下加一条分隔线。
/// 2026-09-07 起渲染层不再用它画表（CJK 落到备用字体时并不是等宽字体的两倍宽，
/// 列会漂）——改成按实际排版测量列宽的网格（messages_view::md_table）；这里留作
/// `plain()` 与测试的纯文本形式。
pub fn table_text(header: &[Vec<Span>], rows: &[Vec<Vec<Span>>]) -> String {
    let cols = rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let cell = |row: &[Vec<Span>], i: usize| -> String {
        row.get(i)
            .map(|s| spans_plain(s).replace('\n', " "))
            .unwrap_or_default()
    };
    let mut widths = vec![0usize; cols];
    for row in std::iter::once(header).chain(rows.iter().map(Vec::as_slice)) {
        for (i, w) in widths.iter_mut().enumerate() {
            *w = (*w).max(display_width(&cell(row, i)));
        }
    }
    let line = |row: &[Vec<Span>]| -> String {
        (0..cols)
            .map(|i| pad(&cell(row, i), widths[i]))
            .collect::<Vec<_>>()
            .join(" | ")
            .trim_end()
            .to_string()
    };
    let mut out = Vec::with_capacity(rows.len() + 2);
    out.push(line(header));
    out.push(
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("-+-"),
    );
    out.extend(rows.iter().map(|r| line(r)));
    out.join("\n")
}

/// 显示宽度：CJK / 全角 / 常见 emoji 记 2 格，其余记 1 格
pub fn display_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

fn char_width(ch: char) -> usize {
    match ch as u32 {
        0x1100..=0x115F // 谚文字母
        | 0x2E80..=0x303E // CJK 部首 / 标点
        | 0x3041..=0x33FF // 假名 / 注音 / 兼容字符
        | 0x3400..=0x4DBF // CJK 扩展 A
        | 0x4E00..=0x9FFF // CJK 统一表意文字
        | 0xA000..=0xA4CF // 彝文
        | 0xAC00..=0xD7A3 // 谚文音节
        | 0xF900..=0xFAFF // CJK 兼容表意文字
        | 0xFE30..=0xFE4F // CJK 兼容形式
        | 0xFF00..=0xFF60 // 全角 ASCII / 标点
        | 0xFFE0..=0xFFE6 // 全角符号
        | 0x1F300..=0x1F64F // emoji
        | 0x1F900..=0x1F9FF // emoji 补充
        | 0x20000..=0x3FFFD => 2, // CJK 扩展 B+
        _ => 1,
    }
}

fn pad(s: &str, width: usize) -> String {
    let mut out = s.to_string();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(display_width(s))));
    out
}

pub fn spans_plain(spans: &[Span]) -> String {
    spans.iter().map(|s| s.text.as_str()).collect()
}

#[cfg_attr(not(test), allow(dead_code))]
fn block_plain(b: &Block) -> String {
    match b {
        Block::Heading { spans, .. } | Block::Paragraph(spans) => spans_plain(spans),
        Block::Code { text, .. } => text.clone(),
        Block::List {
            ordered,
            start,
            items,
        } => items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let marker = if *ordered {
                    format!("{}. ", start + i as u64)
                } else {
                    "- ".to_string()
                };
                // 列表项内部的块紧凑排列（单换行），续行缩进两格对齐标记后的文本
                let body = item.iter().map(block_plain).collect::<Vec<_>>().join("\n");
                format!("{marker}{}", body.replace('\n', "\n  "))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Quote(children) => plain(children)
            .lines()
            .map(|l| format!("> {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::Rule => "---".to_string(),
        Block::Table { header, rows } => table_text(header, rows),
    }
}

// ── 事件 → 块 的构建器 ─────────────────────────────────────────────────────────

/// 容器栈帧。frames[0] 永远是文档根 `Blocks`。
enum Frame {
    /// 文档根 / 列表项 / 引用块的子块容器
    Blocks(Vec<Block>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
    Table {
        header: Vec<Vec<Span>>,
        rows: Vec<Vec<Vec<Span>>>,
        /// 正在收集的当前行（表头也先收在这里）
        row: Vec<Vec<Span>>,
    },
}

struct Builder {
    frames: Vec<Frame>,
    /// 尚未归入块的行内片段（段落 / 标题 / 表格单元格 / 紧凑列表项文本）
    inline: Vec<Span>,
    bold: u32,
    italic: u32,
    strike: u32,
    /// 链接 / 图片嵌套栈，栈顶生效
    links: Vec<String>,
    /// 正在收集的标题级别
    heading: Option<u8>,
    /// 正在收集的代码块 (lang, 正文)
    code: Option<(String, String)>,
    /// 进入图片时 inline 的长度 + URL：结束时若没产生 alt 文本，用 URL 兜底
    image_marks: Vec<(usize, String)>,
}

impl Builder {
    fn new() -> Self {
        Builder {
            frames: vec![Frame::Blocks(Vec::new())],
            inline: Vec::new(),
            bold: 0,
            italic: 0,
            strike: 0,
            links: Vec::new(),
            heading: None,
            code: None,
            image_marks: Vec::new(),
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        // 代码块内只有 Text；其余事件按常规处理（pulldown 不会在代码块里发别的）
        if let Some((_, buf)) = &mut self.code
            && let Event::Text(t) = &ev
        {
            buf.push_str(t);
            return;
        }
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(t) | Event::Html(t) | Event::InlineHtml(t) => self.text(&t),
            Event::Code(t) | Event::InlineMath(t) | Event::DisplayMath(t) => {
                let mut s = self.style();
                s.code = true;
                s.text = t.into_string();
                self.push_span(s);
            }
            Event::FootnoteReference(label) => self.text(&format!("[^{label}]")),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.text("\n"),
            Event::Rule => {
                self.flush_inline();
                self.push_block(Block::Rule);
            }
            Event::TaskListMarker(done) => self.text(if done { "☑ " } else { "☐ " }),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph | Tag::HtmlBlock => self.flush_inline(),
            Tag::Heading { level, .. } => {
                self.flush_inline();
                self.heading = Some(level as u8);
            }
            Tag::BlockQuote(_) | Tag::Item => {
                self.flush_inline();
                self.frames.push(Frame::Blocks(Vec::new()));
            }
            Tag::CodeBlock(kind) => {
                self.flush_inline();
                let lang = match kind {
                    // info string 只取第一个词（```rust title=x → rust）
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().unwrap_or("").to_string()
                    }
                    CodeBlockKind::Indented => String::new(),
                };
                self.code = Some((lang, String::new()));
            }
            Tag::List(start) => {
                self.flush_inline();
                self.frames.push(Frame::List {
                    ordered: start.is_some(),
                    start: start.unwrap_or(1),
                    items: Vec::new(),
                });
            }
            Tag::Table(_) => {
                self.flush_inline();
                self.frames.push(Frame::Table {
                    header: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(Frame::Table { row, .. }) = self.frames.last_mut() {
                    row.clear();
                }
            }
            Tag::TableCell => {}
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.links.push(dest_url.into_string()),
            Tag::Image { dest_url, .. } => {
                let url = dest_url.into_string();
                self.image_marks.push((self.inline.len(), url.clone()));
                self.links.push(url);
            }
            // 未启用 / 不支持的容器：内容照常以文本流出
            Tag::FootnoteDefinition(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::MetadataBlock(_) => self.flush_inline(),
            Tag::Superscript | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock => self.flush_inline(),
            TagEnd::Heading(_) => {
                let level = self.heading.take().unwrap_or(1);
                let spans = self.take_inline();
                if !spans.is_empty() {
                    self.push_block(Block::Heading { level, spans });
                }
            }
            TagEnd::BlockQuote(_) => {
                self.flush_inline();
                if let Some(children) = self.pop_blocks() {
                    self.push_block(Block::Quote(children));
                }
            }
            TagEnd::CodeBlock => {
                if let Some((lang, mut text)) = self.code.take() {
                    if text.ends_with('\n') {
                        text.pop();
                    }
                    self.push_block(Block::Code { lang, text });
                }
            }
            TagEnd::List(_) => {
                self.flush_inline();
                if self.frames.len() > 1
                    && matches!(self.frames.last(), Some(Frame::List { .. }))
                    && let Some(Frame::List {
                        ordered,
                        start,
                        items,
                    }) = self.frames.pop()
                {
                    self.push_block(Block::List {
                        ordered,
                        start,
                        items,
                    });
                }
            }
            TagEnd::Item => {
                self.flush_inline();
                if let Some(children) = self.pop_blocks() {
                    self.push_item(children);
                }
            }
            TagEnd::Table => {
                self.flush_inline();
                if self.frames.len() > 1
                    && matches!(self.frames.last(), Some(Frame::Table { .. }))
                    && let Some(Frame::Table { header, rows, .. }) = self.frames.pop()
                {
                    self.push_block(Block::Table { header, rows });
                }
            }
            TagEnd::TableHead => {
                if let Some(Frame::Table { header, row, .. }) = self.frames.last_mut() {
                    *header = std::mem::take(row);
                }
            }
            TagEnd::TableRow => {
                if let Some(Frame::Table { rows, row, .. }) = self.frames.last_mut() {
                    rows.push(std::mem::take(row));
                }
            }
            TagEnd::TableCell => {
                let spans = self.take_inline();
                if let Some(Frame::Table { row, .. }) = self.frames.last_mut() {
                    row.push(spans);
                } else {
                    // 表格帧不在栈顶（畸形输入）：文本不丢，留在行内缓冲
                    self.inline = spans;
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
            }
            TagEnd::Image => {
                self.links.pop();
                if let Some((mark, url)) = self.image_marks.pop()
                    && self.inline.len() == mark
                {
                    // 没有 alt 文本：用 URL 兜底，至少可见可点
                    self.push_span(Span {
                        text: url.clone(),
                        link: Some(url),
                        ..Span::default()
                    });
                }
            }
            TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::MetadataBlock(_) => self.flush_inline(),
            TagEnd::Superscript | TagEnd::Subscript => {}
        }
    }

    fn finish(mut self) -> Vec<Block> {
        self.flush_inline();
        if let Some((lang, text)) = self.code.take() {
            self.push_block(Block::Code { lang, text });
        }
        // 未闭合的容器（理论上 pulldown 会全部闭合）：逐层收拢，不丢内容
        while self.frames.len() > 1 {
            match self.frames.pop() {
                Some(Frame::Blocks(children)) => self.push_item(children),
                Some(Frame::List {
                    ordered,
                    start,
                    items,
                }) => self.push_block(Block::List {
                    ordered,
                    start,
                    items,
                }),
                Some(Frame::Table { header, rows, .. }) => {
                    self.push_block(Block::Table { header, rows })
                }
                None => break,
            }
        }
        match self.frames.pop() {
            Some(Frame::Blocks(v)) => v,
            _ => Vec::new(),
        }
    }

    // ── 辅助 ──

    /// 当前行内样式（不含文本）
    fn style(&self) -> Span {
        Span {
            text: String::new(),
            bold: self.bold > 0,
            italic: self.italic > 0,
            code: false,
            strike: self.strike > 0,
            link: self.links.last().cloned(),
        }
    }

    fn text(&mut self, t: &str) {
        if t.is_empty() {
            return;
        }
        let mut s = self.style();
        s.text = t.to_string();
        self.push_span(s);
    }

    /// 追加片段，样式相同则并入上一片段
    fn push_span(&mut self, s: Span) {
        if s.text.is_empty() {
            return;
        }
        if let Some(last) = self.inline.last_mut()
            && last.same_style(&s)
        {
            last.text.push_str(&s.text);
        } else {
            self.inline.push(s);
        }
    }

    /// 取走行内缓冲；去掉末尾空白（HTML 块 / 软换行残留），丢弃空片段
    fn take_inline(&mut self) -> Vec<Span> {
        let mut spans = std::mem::take(&mut self.inline);
        while let Some(last) = spans.last_mut() {
            if !last.code {
                let trimmed = last.text.trim_end();
                if trimmed.len() != last.text.len() {
                    last.text = trimmed.to_string();
                }
            }
            if last.text.is_empty() {
                spans.pop();
            } else {
                break;
            }
        }
        spans
    }

    /// 行内缓冲非空 → 作为段落落到当前容器（紧凑列表项、HTML 块等无 Paragraph 标签的场合）
    fn flush_inline(&mut self) {
        let spans = self.take_inline();
        if !spans.is_empty() {
            self.push_block(Block::Paragraph(spans));
        }
    }

    /// 落块到最近的 Blocks 容器（跳过 List / Table 帧：畸形输入时也不丢）
    fn push_block(&mut self, b: Block) {
        if let Some(Frame::Blocks(v)) = self
            .frames
            .iter_mut()
            .rev()
            .find(|f| matches!(f, Frame::Blocks(_)))
        {
            v.push(b);
        }
    }

    /// 栈顶是子块容器且不是根时弹出
    fn pop_blocks(&mut self) -> Option<Vec<Block>> {
        if self.frames.len() > 1
            && matches!(self.frames.last(), Some(Frame::Blocks(_)))
            && let Some(Frame::Blocks(children)) = self.frames.pop()
        {
            return Some(children);
        }
        None
    }

    /// 列表项内容归入栈顶列表；栈顶不是列表（畸形）就平铺进父容器
    fn push_item(&mut self, children: Vec<Block>) {
        if let Some(Frame::List { items, .. }) = self.frames.last_mut() {
            items.push(children);
        } else {
            for b in children {
                self.push_block(b);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(text: &str) -> Span {
        Span {
            text: text.to_string(),
            ..Span::default()
        }
    }

    fn para(text: &str) -> Block {
        Block::Paragraph(vec![span(text)])
    }

    #[test]
    fn headings_h1_to_h3() {
        let blocks = parse("# One\n## Two\n### Three\n");
        assert_eq!(
            blocks,
            vec![
                Block::Heading {
                    level: 1,
                    spans: vec![span("One")]
                },
                Block::Heading {
                    level: 2,
                    spans: vec![span("Two")]
                },
                Block::Heading {
                    level: 3,
                    spans: vec![span("Three")]
                },
            ]
        );
    }

    #[test]
    fn paragraph_inline_styles() {
        let blocks = parse("**bold** *italic* `code` [link](https://x.y/z) ~~strike~~");
        let Some(Block::Paragraph(spans)) = blocks.first() else {
            panic!("expected paragraph, got {blocks:?}");
        };
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            spans.as_slice(),
            &[
                Span {
                    text: "bold".into(),
                    bold: true,
                    ..Span::default()
                },
                span(" "),
                Span {
                    text: "italic".into(),
                    italic: true,
                    ..Span::default()
                },
                span(" "),
                Span {
                    text: "code".into(),
                    code: true,
                    ..Span::default()
                },
                span(" "),
                Span {
                    text: "link".into(),
                    link: Some("https://x.y/z".into()),
                    ..Span::default()
                },
                span(" "),
                Span {
                    text: "strike".into(),
                    strike: true,
                    ..Span::default()
                },
            ]
        );
        assert_eq!(plain(&blocks), "bold italic code link strike");
    }

    #[test]
    fn nested_inline_styles_combine() {
        let blocks = parse("***both*** and **[bold link](u)**");
        let Some(Block::Paragraph(spans)) = blocks.first() else {
            panic!("expected paragraph");
        };
        assert!(spans[0].bold && spans[0].italic && spans[0].text == "both");
        let link = spans.iter().find(|s| s.link.is_some()).expect("link span");
        assert!(link.bold);
        assert_eq!(link.text, "bold link");
        assert_eq!(link.link.as_deref(), Some("u"));
    }

    #[test]
    fn fenced_code_with_language() {
        let blocks = parse("before\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nafter");
        assert_eq!(
            blocks,
            vec![
                para("before"),
                Block::Code {
                    lang: "rust".into(),
                    text: "fn main() {\n    println!(\"hi\");\n}".into(),
                },
                para("after"),
            ]
        );
        // 无语言的围栏 + 缩进代码块
        assert_eq!(
            parse("```\nx\n```"),
            vec![Block::Code {
                lang: String::new(),
                text: "x".into()
            }]
        );
        assert_eq!(
            parse("    indented\n    code\n"),
            vec![Block::Code {
                lang: String::new(),
                text: "indented\ncode".into()
            }]
        );
    }

    #[test]
    fn nested_bullets_and_ordered_start() {
        let blocks = parse("- a\n  - b\n  - c\n- d\n\n3. x\n4. y\n");
        assert_eq!(
            blocks,
            vec![
                Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![
                        vec![
                            para("a"),
                            Block::List {
                                ordered: false,
                                start: 1,
                                items: vec![vec![para("b")], vec![para("c")]],
                            },
                        ],
                        vec![para("d")],
                    ],
                },
                Block::List {
                    ordered: true,
                    start: 3,
                    items: vec![vec![para("x")], vec![para("y")]],
                },
            ]
        );
        assert_eq!(plain(&blocks), "- a\n  - b\n  - c\n- d\n\n3. x\n4. y");
    }

    #[test]
    fn loose_list_items_keep_paragraphs() {
        // 松散列表（项间空行）每项都有 Paragraph 标签，结果应与紧凑列表同构
        let blocks = parse("1. one\n\n2. two\n");
        assert_eq!(
            blocks,
            vec![Block::List {
                ordered: true,
                start: 1,
                items: vec![vec![para("one")], vec![para("two")]],
            }]
        );
    }

    #[test]
    fn blockquote_with_soft_break() {
        let blocks = parse("> hello\n> world\n>\n> - item");
        assert_eq!(
            blocks,
            vec![Block::Quote(vec![
                para("hello world"),
                Block::List {
                    ordered: false,
                    start: 1,
                    items: vec![vec![para("item")]],
                },
            ])]
        );
        assert_eq!(plain(&blocks), "> hello world\n> \n> - item");
    }

    #[test]
    fn rule_between_paragraphs() {
        assert_eq!(
            parse("a\n\n---\n\nb"),
            vec![para("a"), Block::Rule, para("b")]
        );
    }

    #[test]
    fn hard_break_stays_in_paragraph() {
        assert_eq!(parse("line one  \nline two"), vec![para("line one\nline two")]);
        assert_eq!(parse("soft\nbreak"), vec![para("soft break")]);
    }

    #[test]
    fn gfm_table() {
        let blocks = parse("| Name | 数量 |\n|---|---:|\n| foo | 1 |\n| 中文 | 22 |\n");
        assert_eq!(
            blocks,
            vec![Block::Table {
                header: vec![vec![span("Name")], vec![span("数量")]],
                rows: vec![
                    vec![vec![span("foo")], vec![span("1")]],
                    vec![vec![span("中文")], vec![span("22")]],
                ],
            }]
        );
        assert_eq!(
            plain(&blocks),
            "Name | 数量\n-----+-----\nfoo  | 1\n中文 | 22"
        );
    }

    #[test]
    fn display_width_counts_cjk_double() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("中文ab"), 6);
        assert_eq!(display_width("ＡＢ"), 4);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn bare_url_stays_single_paragraph() {
        let src = "see https://example.com/x?y=1&z=2 now, and mail me: a@b.c";
        let blocks = parse(src);
        assert_eq!(blocks, vec![para(src)]);
        assert_eq!(plain(&blocks), src);
    }

    #[test]
    fn image_alt_becomes_link_span() {
        assert_eq!(
            parse("![diagram](https://i.example/d.png)"),
            vec![Block::Paragraph(vec![Span {
                text: "diagram".into(),
                link: Some("https://i.example/d.png".into()),
                ..Span::default()
            }])]
        );
        // 空 alt → URL 兜底
        assert_eq!(
            parse("![](https://i.example/d.png)"),
            vec![Block::Paragraph(vec![Span {
                text: "https://i.example/d.png".into(),
                link: Some("https://i.example/d.png".into()),
                ..Span::default()
            }])]
        );
    }

    #[test]
    fn html_degrades_to_literal_text() {
        assert_eq!(parse("a <b>x</b> c"), vec![para("a <b>x</b> c")]);
        assert_eq!(
            parse("<div>\nblock\n</div>\n"),
            vec![para("<div>\nblock\n</div>")]
        );
    }

    #[test]
    fn empty_and_whitespace_give_no_blocks() {
        assert!(parse("").is_empty());
        assert!(parse("   \n\n  ").is_empty());
    }

    #[test]
    fn odd_input_never_panics() {
        for src in [
            "```",
            "```rust\nunterminated",
            "> - * # ",
            "| a |\n|",
            "| a | b |\n|---|\n| 1 |",
            "[](",
            "***",
            "- \n- \n  - ",
            "1.\n2.",
            "\u{0}\u{1}\u{7f}",
            "# \n## \n",
            "![](",
            "<",
            "  \t  ",
            "> > > > deep\n>>>> er",
            "* a\n\n    code in list\n\n* b",
        ] {
            let blocks = parse(src);
            let _ = plain(&blocks);
        }
    }
}
