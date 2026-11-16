//! File-pane overlays follow the viewport through live values, without view commits.
use crate::app::App;
use rune::{Color, El, Live, Phase, ScrollPos, col, el};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

pub struct Chrome {
    rect: Live<[f32; 4]>,
    opacity: Live<f32>,
    position: Cell<ScrollPos>,
    mouse: Cell<bool>,
}
impl Default for Chrome {
    fn default() -> Self {
        Self {
            rect: Live::new([0.0; 4]),
            opacity: Live::new(0.0),
            position: Cell::new(ScrollPos::default()),
            mouse: Cell::new(false),
        }
    }
}
impl Chrome {
    pub fn hide_keyboard(&self) {
        self.mouse.set(false);
        self.opacity.snap(0.0);
    }
    fn pulse(&self) {
        self.opacity.pulse(1.0, 0.0, Duration::from_millis(1500));
    }
    fn follow(&self, pos: ScrollPos) {
        let previous = self.position.replace(pos);
        if pos.max[1] <= 0.0 {
            self.opacity.snap(0.0);
            self.rect.snap([0.0; 4]);
            return;
        }
        let track = (pos.drawn[1] - 8.0).max(0.0);
        let height = (track * pos.viewport[1] / (pos.viewport[1] + pos.max[1]))
            .max(28.0)
            .min(track);
        let y = 4.0 + (track - height) * pos.offset[1] / pos.max[1];
        self.rect
            .snap([(pos.drawn[0] - 9.0).max(0.0), y, 5.0, height]);
        if self.mouse.get() && pos.offset[1] != previous.offset[1] {
            self.pulse();
        }
    }
}
pub fn wrap(viewport: El, key: String, color: Color, chrome: Rc<Chrome>) -> El {
    let following = chrome.clone();
    let wheel = chrome.clone();
    let drag = chrome.clone();
    let viewport = viewport
        .chain_scroll_offset(move |pos| following.follow(pos))
        .on_wheel(move |_: &mut App, _, tx| {
            tx.live_only();
            wheel.mouse.set(true);
            if wheel.position.get().max[1] > 0.0 {
                wheel.pulse();
            }
            false
        });
    let target = key.clone();
    col()
        .key(format!("{key}-scroll-area"))
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(viewport)
        .child(
            el().key(format!("{key}-thumb"))
                .role("scrollbar")
                .label("Scroll files")
                .focusable(false)
                .pin(Some(0.0), Some(0.0), None, None)
                .size(5.0, 28.0)
                .radius(3.0)
                .fill(color)
                .live_rect(&chrome.rect)
                .live_opacity(chrome.opacity.smooth(rune::tok::motion::SNAPPY))
                .on_drag_any(move |gesture, tx| {
                    tx.live_only();
                    let pos = drag.position.get();
                    match gesture.phase {
                        Phase::Down => {
                            gesture.data = pos.offset[1];
                            drag.mouse.set(true);
                            drag.opacity.snap(1.0);
                        }
                        Phase::Move => {
                            let track = (pos.drawn[1] - 8.0 - drag.rect.get()[3]).max(1.0);
                            tx.scroll_to_instant(
                                target.clone(),
                                0.0,
                                (gesture.data + gesture.dy * pos.max[1] / track)
                                    .clamp(0.0, pos.max[1]),
                            );
                        }
                        Phase::Up => drag.pulse(),
                    }
                }),
        )
}
