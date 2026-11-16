//! ePubViewer 的 Rust 侧：`App.tsx` 从 `epubviewer/native` 导入这里的 `Epub`
//! 模块，以及由 `#[derive(JsObject)]` 生成的 `Book` / `Chapter` / `TocEntry`。

mod epub;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use nsh_base as _;
use nsh_core::file_dialog::{FileFilter, OpenDialog};
use nsh_core::prelude::*;

/// 当前打开的书。章节正文按需从这里取，避免把整本书一次传给 WebView。
static LOADED: OnceLock<Mutex<Option<Arc<epub::Loaded>>>> = OnceLock::new();

#[native_module("epubviewer/native")]
fn native(registry: &mut Registry) {
    registry.module::<Epub>();
}

fn store(loaded: epub::Loaded) -> epub::Book {
    let book = loaded.book.clone();
    let lock = LOADED.get_or_init(|| Mutex::new(None));
    *lock.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(loaded));
    book
}

/// EPUB 文件的解析入口。
#[module]
pub struct Epub;

#[module]
impl Epub {
    /// 弹出系统文件对话框，读取并解析一本 EPUB；取消时返回 null。
    pub async fn open() -> Result<Option<epub::Book>, String> {
        let paths = pick_paths(OpenDialog::files().filter(FileFilter::new("EPUB", ["epub"]))).await?;
        let Some(path) = paths.and_then(|picked| picked.into_iter().next()) else {
            return Ok(None);
        };
        parse_at(path).await.map(Some)
    }

    /// 打开一个本地路径（把文件拖到窗口时使用，也接受 `file://` URL）。
    pub async fn load(path: String) -> Result<epub::Book, String> {
        let path = path.strip_prefix("file://").unwrap_or(&path);
        parse_at(PathBuf::from(epub::percent_decode(path))).await
    }

    /// 解析 EPUB 文件的字节。
    pub async fn parse(bytes: Vec<u8>) -> Result<epub::Book, String> {
        parse_bytes(bytes).await
    }

    /// 取指定章节的正文 HTML；只传这一章，WebView 才装得下。
    pub async fn chapter(index: u32) -> Result<String, String> {
        let loaded = LOADED
            .get()
            .and_then(|lock| lock.lock().unwrap_or_else(|e| e.into_inner()).clone())
            .ok_or_else(|| "还没有打开任何 EPUB".to_owned())?;
        blocking(move || loaded.chapter_html(index))
            .await
            .map_err(|e| format!("读取章节失败: {e}"))
    }
}

async fn parse_at(path: PathBuf) -> Result<epub::Book, String> {
    let bytes = blocking(move || {
        std::fs::read(&path).map_err(|e| format!("无法读取 {}: {e}", path.display()))
    })
    .await
    .map_err(|e| format!("读取任务失败: {e}"))??;
    parse_bytes(bytes).await
}

async fn parse_bytes(bytes: Vec<u8>) -> Result<epub::Book, String> {
    let loaded = blocking(move || epub::parse(bytes))
        .await
        .map_err(|e| format!("解析任务失败: {e}"))??;
    Ok(store(loaded))
}
