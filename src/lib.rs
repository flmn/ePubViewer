//! ePubViewer 的 Rust 侧：`App.tsx` 从 `epubviewer/native` 导入这里的 `Epub`
//! 模块，以及由 `#[derive(JsObject)]` 生成的 `Book` / `Chapter` / `TocEntry`。

mod epub;

use std::path::PathBuf;

use nsh_base as _;
use nsh_core::file_dialog::{FileFilter, OpenDialog};
use nsh_core::prelude::*;

#[native_module("epubviewer/native")]
fn native(registry: &mut Registry) {
    registry.module::<Epub>();
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
    blocking(move || epub::parse(bytes))
        .await
        .map_err(|e| format!("解析任务失败: {e}"))?
}
