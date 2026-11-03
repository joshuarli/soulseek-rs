use crate::models::DownloadEntry;
use soulseek_rs::types::UploadInfo;

pub enum Selection<'a> {
    Download(&'a DownloadEntry),
    Upload(&'a UploadInfo),
}

/// Resolve the transfers table's selected row index into the download or
/// upload it points at, matching the order `render_downloads_pane` renders.
#[must_use]
pub fn selected_transfer<'a>(
    index: Option<usize>,
    downloads: &'a [DownloadEntry],
    uploads: &'a [UploadInfo],
) -> Option<Selection<'a>> {
    let index = index?;
    if let Some(download) = downloads.get(index) {
        return Some(Selection::Download(download));
    }
    // Past the downloads, so this row is an upload.
    uploads
        .get(index - downloads.len())
        .map(Selection::Upload)
}
