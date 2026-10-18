//! Full-window image viewer for the images of one message.

use image::DynamicImage;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect, Size};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::{Image, Resize};
use slack_core::types::File;

use super::text::{self, DIM};

pub enum Loaded {
    Loading,
    Ready(DynamicImage),
    Failed(String),
}

pub struct Viewer {
    pub files: Vec<File>,
    pub index: usize,
    pub image: Loaded,
    /// The image encoded for the last inner area it was drawn in.
    encoded: Option<(Size, Protocol)>,
}

impl Viewer {
    pub fn new(files: Vec<File>, index: usize) -> Viewer {
        Viewer { files, index, image: Loaded::Loading, encoded: None }
    }

    pub fn file(&self) -> &File {
        &self.files[self.index]
    }

    /// Show the image at `index + delta` (wrapping); returns true if it changed.
    pub fn step(&mut self, delta: isize) -> bool {
        let n = self.files.len() as isize;
        let next = (self.index as isize + delta).rem_euclid(n.max(1)) as usize;
        if next == self.index {
            return false;
        }
        self.index = next;
        self.image = Loaded::Loading;
        self.encoded = None;
        true
    }

    pub fn set_image(&mut self, file_id: &str, image: Result<DynamicImage, String>) {
        if self.file().id == file_id {
            self.image = match image {
                Ok(img) => Loaded::Ready(img),
                Err(e) => Loaded::Failed(e),
            };
            self.encoded = None;
        }
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect, picker: &Picker) {
        f.render_widget(Clear, area);
        let file = self.file().clone();
        let name = if file.title.is_empty() { file.name.clone() } else { file.title.clone() };
        let count = if self.files.len() > 1 { format!(" · {}/{}", self.index + 1, self.files.len()) } else { String::new() };
        let title = format!(" {name} · {}{count} ", text::human_size(file.size));
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Rgb(0x61, 0xaf, 0xef))).title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);
        match &self.image {
            Loaded::Loading => f.render_widget(Paragraph::new("Loading image…").style(Style::default().fg(DIM)), inner),
            Loaded::Failed(e) => f.render_widget(Paragraph::new(format!("Could not load the image: {e}")).style(Style::default().fg(DIM)), inner),
            Loaded::Ready(img) => {
                let size = Size::new(inner.width, inner.height);
                if self.encoded.as_ref().is_none_or(|(s, _)| *s != size) {
                    self.encoded = picker.new_protocol(img.clone(), size, Resize::Fit(None)).ok().map(|p| (size, p));
                }
                if let Some((_, proto)) = &self.encoded {
                    let fit = proto.size();
                    let [x] = Layout::horizontal([Constraint::Length(fit.width)]).flex(Flex::Center).areas(inner);
                    let [cell] = Layout::vertical([Constraint::Length(fit.height)]).flex(Flex::Center).areas(x);
                    f.render_widget(Image::new(proto), cell);
                }
            }
        }
    }
}

/// Download and decode the large version of an image file.
pub async fn load_full(client: slack_core::Client, file: File) -> anyhow::Result<DynamicImage> {
    let url = file.thumb_1024.as_deref().or(file.url_private.as_deref()).or(file.preview_url(2048)).ok_or_else(|| anyhow::anyhow!("no image URL"))?.to_string();
    let bytes = client.download(&url).await?;
    Ok(tokio::task::spawn_blocking(move || image::load_from_memory(&bytes)).await??)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepping_wraps_and_resets() {
        let files: Vec<File> = (0..3).map(|i| File { id: format!("F{i}"), ..Default::default() }).collect();
        let mut v = Viewer::new(files, 0);
        v.set_image("F0", Err("x".into()));
        assert!(v.step(-1));
        assert_eq!(v.index, 2);
        assert!(matches!(v.image, Loaded::Loading));
        // A late result for another file is ignored.
        v.set_image("F0", Err("late".into()));
        assert!(matches!(v.image, Loaded::Loading));
        let mut single = Viewer::new(vec![File::default()], 0);
        assert!(!single.step(1));
    }
}
