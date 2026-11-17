//! ePubViewer 的 Rust 侧：`App.tsx` 从 `epubviewer/native` 导入这里的 `Epub`
//! 模块，以及由 `#[derive(JsObject)]` 生成的 `Book` / `Chapter` / `TocEntry`。

mod epub;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use nsh_base as _;
use nsh_core::file_dialog::{FileFilter, OpenDialog};
use nsh_core::prelude::*;

/// 当前打开的书。原始字节留在这里：整书文档按需拼装，资源按需取字节。
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

    /// 整本书的单文档 HTML：所有章节按 spine 顺序拼好，脚本负责分页与
    /// 上报页码。图片等资源是 `epub://` 占位，用 `Epub.resource()` 换成
    /// `data:` URL 再填回页面。
    pub async fn book(theme: epub::BookTheme) -> Result<String, String> {
        let loaded = current()?;
        blocking(move || loaded.book_html(&theme))
            .await
            .map_err(|e| format!("排版失败: {e}"))
    }

    /// 把文档里的 `epub://OEBPS/images/a.png` 换成可直接当 `src` 用的
    /// `data:` URL；EPUB 里没有这个条目时返回 null。
    pub async fn resource(url: String) -> Result<Option<String>, String> {
        let loaded = current()?;
        blocking(move || loaded.resource(&url))
            .await
            .map_err(|e| format!("读取资源失败: {e}"))
    }
}

/// 当前打开的书。
fn current() -> Result<Arc<epub::Loaded>, String> {
    LOADED
        .get()
        .and_then(|lock| lock.lock().unwrap_or_else(|e| e.into_inner()).clone())
        .ok_or_else(|| "还没有打开任何 EPUB".to_owned())
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
