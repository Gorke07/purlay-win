//! Attachment pictures, read once through the session and kept for the window's life
//! (bounded: 64 items / 48 MB, oldest dropped first).

use crate::state::AppModel;
use gpui_kit::*;
use purlay_core::contract::Uuid;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

const MAX_ITEMS: usize = 64;
const MAX_BYTES: usize = 48 * 1024 * 1024;

pub enum Slot {
    Loading,
    Ready(Arc<Image>),
    Failed,
}

pub struct Images {
    app: Entity<AppModel>,
    slots: HashMap<(Uuid, bool), Slot>,
    order: VecDeque<(Uuid, bool)>,
    bytes: usize,
}

pub struct ImagesGlobal(pub Entity<Images>);
impl Global for ImagesGlobal {}

pub fn images(cx: &App) -> Entity<Images> {
    cx.global::<ImagesGlobal>().0.clone()
}

impl Images {
    pub fn new(app: Entity<AppModel>) -> Self {
        Images { app, slots: HashMap::new(), order: VecDeque::new(), bytes: 0 }
    }

    /// The picture if it is here; otherwise starts reading it and returns `None`.
    pub fn get(&mut self, id: &Uuid, thumbnail: bool, cx: &mut Context<Self>) -> Option<Arc<Image>> {
        let key = (id.clone(), thumbnail);
        match self.slots.get(&key) {
            Some(Slot::Ready(i)) => return Some(i.clone()),
            Some(Slot::Loading) | Some(Slot::Failed) => return None,
            None => {}
        }
        let session = self.app.read(cx).session().cloned()?;
        self.slots.insert(key.clone(), Slot::Loading);
        let h = session.attachment_bytes(id, thumbnail);
        cx.spawn(async move |this, cx| {
            let r = h.await;
            let _ = this.update(cx, |me, cx| {
                match r {
                    Ok(Ok(bytes)) => {
                        let format = sniff(&bytes);
                        let len = bytes.len();
                        let img = Arc::new(Image::from_bytes(format, bytes.to_vec()));
                        me.slots.insert(key.clone(), Slot::Ready(img));
                        me.order.push_back(key);
                        me.bytes += len;
                        me.evict();
                    }
                    _ => {
                        me.slots.insert(key, Slot::Failed);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        None
    }

    fn evict(&mut self) {
        while self.order.len() > MAX_ITEMS || self.bytes > MAX_BYTES {
            let Some(k) = self.order.pop_front() else { break };
            if let Some(Slot::Ready(i)) = self.slots.remove(&k) {
                self.bytes = self.bytes.saturating_sub(i.bytes.len());
            }
        }
    }
}

pub fn sniff(b: &[u8]) -> ImageFormat {
    if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        ImageFormat::Png
    } else if b.starts_with(&[0xff, 0xd8]) {
        ImageFormat::Jpeg
    } else if b.starts_with(b"GIF8") {
        ImageFormat::Gif
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else if b.starts_with(b"<svg") || b.starts_with(b"<?xml") {
        ImageFormat::Svg
    } else if b.starts_with(b"BM") {
        ImageFormat::Bmp
    } else {
        ImageFormat::Png
    }
}
