//! JUMP MODE: one key labels everything on the grid you can land on — every
//! session card, every BAND's rule and every PROJECT TAB — and typing a
//! label lands there, as
//! flash.nvim, EasyMotion and Vimium's link hints do. Any card on screen is
//! two or three keys away, however far the cursor would have to walk.
//!
//! The labels are worked out once, when the mode opens, off what the last
//! frame drew (`App::hits`), and held by identity — the session, the
//! checkout, the project — never by place: a card that moves while the labels are up
//! (a status change resorting the band) keeps its label, and the label is
//! drawn wherever the card is now.
//!
//! This module is the model and the drawing; the keys are
//! `event_loop::jump`'s.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::Frame;

use nebula_core::{ProjectId, SessionRef, WorktreeId};

use crate::app::{App, HitTarget};

/// Labels are drawn from this alphabet, home row first: the keys under the
/// fingers go to the first targets, which are the ones at the top left.
const ALPHABET: &[u8] = b"asdfghjklqwertyuiopzxcvbnm";

/// Somewhere a label can take you.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JumpTarget {
    /// A session or terminal card on the grid.
    Card(SessionRef),
    /// A BAND's rule — the checkout's own row over its cards, the way onto
    /// a band whose cards are folded away or below the fold.
    Band(WorktreeId),
    /// A PROJECT TAB in the header.
    Tab(ProjectId),
}

#[derive(Debug, Clone)]
pub struct JumpView {
    /// Every label and where it goes, in screen order.
    pub labels: Vec<(String, JumpTarget)>,
    /// What has been typed of a label so far.
    pub typed: String,
}

/// What a typed character did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Typed {
    /// A label starts with what has been typed: wait for the next key.
    Pending,
    /// A label was typed in full.
    Landed(JumpTarget),
    /// No label starts with it.
    Missed,
}

/// `n` labels, prefix-free: one letter each while the alphabet covers
/// them, two letters each past that. Never a mix, so a one-letter label
/// can never be the start of another.
pub fn labels(n: usize) -> Vec<String> {
    let one = |c: u8| (c as char).to_string();
    if n <= ALPHABET.len() {
        return ALPHABET.iter().take(n).map(|&c| one(c)).collect();
    }
    ALPHABET
        .iter()
        .flat_map(|&a| {
            ALPHABET
                .iter()
                .map(move |&b| format!("{}{}", a as char, b as char))
        })
        .take(n)
        .collect()
}

impl JumpView {
    /// The labels for what the last frame drew, top to bottom and left to
    /// right. None when there is nothing to land on.
    pub fn from_screen(app: &App) -> Option<Self> {
        let bands = crate::launcher::bands(app);
        let mut targets: Vec<(Rect, JumpTarget)> = Vec::new();
        for (rect, hit) in &app.hits {
            let target = match hit {
                HitTarget::LauncherCard(at) => {
                    let Some(card) = crate::launcher::card_at(&bands, *at) else {
                        continue;
                    };
                    JumpTarget::Card(card.sref())
                }
                HitTarget::LauncherBand(index) => match bands.get(*index) {
                    Some(band) => JumpTarget::Band(band.worktree.clone()),
                    None => continue,
                },
                HitTarget::LauncherTab(id) => JumpTarget::Tab(id.clone()),
                _ => continue,
            };
            // A target registered twice (a card cut by the pane's edge)
            // gets one label.
            if !targets.iter().any(|(_, t)| *t == target) {
                targets.push((*rect, target));
            }
        }
        if targets.is_empty() {
            return None;
        }
        targets.sort_by_key(|(rect, _)| (rect.y, rect.x));
        let labels = labels(targets.len())
            .into_iter()
            .zip(targets.into_iter().map(|(_, target)| target))
            .collect();
        Some(Self {
            labels,
            typed: String::new(),
        })
    }

    /// `c` typed onto what was typed before it.
    pub fn type_char(&mut self, c: char) -> Typed {
        self.typed.push(c.to_ascii_lowercase());
        let mut live = self
            .labels
            .iter()
            .filter(|(label, _)| label.starts_with(&self.typed));
        match (live.next(), live.next()) {
            (None, _) => Typed::Missed,
            (Some((label, target)), None) if *label == self.typed => Typed::Landed(target.clone()),
            _ => Typed::Pending,
        }
    }
}

/// Where `target` was drawn on this frame, if it is still on screen.
fn rect_of(app: &App, bands: &[crate::launcher::Band], target: &JumpTarget) -> Option<Rect> {
    app.hits.iter().find_map(|(rect, hit)| {
        let here = match (hit, target) {
            (HitTarget::LauncherCard(at), JumpTarget::Card(sref)) => {
                crate::launcher::card_at(bands, *at).is_some_and(|card| card.sref() == *sref)
            }
            (HitTarget::LauncherBand(index), JumpTarget::Band(want)) => {
                bands.get(*index).is_some_and(|band| band.worktree == *want)
            }
            (HitTarget::LauncherTab(id), JumpTarget::Tab(want)) => id == want,
            _ => false,
        };
        here.then_some(*rect)
    })
}

/// The labels over the frame just drawn: each on its target's top-left
/// corner, the part already typed dimmed, and the labels it ruled out
/// gone.
pub fn draw(f: &mut Frame, app: &App, view: &JumpView) {
    let th = app.theme;
    let bands = crate::launcher::bands(app);
    let screen = f.area();
    let typed = Style::default().fg(th.dim).bg(th.accent);
    let rest = Style::default()
        .fg(th.on_accent)
        .bg(th.accent)
        .add_modifier(Modifier::BOLD);
    let buf = f.buffer_mut();
    for (label, target) in &view.labels {
        if !label.starts_with(&view.typed) {
            continue;
        }
        let Some(rect) = rect_of(app, &bands, target) else {
            continue;
        };
        // A card's corner is its border's: one in, onto the top rule, so
        // the label never covers the session's name.
        let x = match target {
            JumpTarget::Card(_) => rect.x.saturating_add(1),
            JumpTarget::Band(_) | JumpTarget::Tab(_) => rect.x,
        };
        if x >= screen.right() || rect.y >= screen.bottom() {
            continue;
        }
        let (done, todo) = label.split_at(view.typed.len());
        let room = usize::from(screen.right() - x);
        buf.set_stringn(x, rect.y, done, room, typed);
        let x = x.saturating_add(done.len() as u16);
        if x < screen.right() {
            buf.set_stringn(x, rect.y, todo, usize::from(screen.right() - x), rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_one_letter_until_the_alphabet_runs_out() {
        assert_eq!(labels(3), ["a", "s", "d"]);
        assert_eq!(labels(26).len(), 26);
        let many = labels(27);
        assert_eq!(many.len(), 27);
        assert!(many.iter().all(|l| l.len() == 2), "never a mix: {many:?}");
        assert_eq!(&many[..2], ["aa", "as"]);
    }

    #[test]
    fn labels_are_prefix_free_and_unique() {
        for n in [1, 26, 27, 200] {
            let all = labels(n);
            for (i, a) in all.iter().enumerate() {
                for (j, b) in all.iter().enumerate() {
                    if i != j {
                        assert!(!b.starts_with(a.as_str()), "{a} is a prefix of {b}");
                    }
                }
            }
        }
    }

    fn view(n: usize) -> JumpView {
        JumpView {
            labels: labels(n)
                .into_iter()
                .enumerate()
                .map(|(i, l)| (l, JumpTarget::Tab(ProjectId(format!("p{i}")))))
                .collect(),
            typed: String::new(),
        }
    }

    #[test]
    fn a_full_label_lands_and_a_stray_key_misses() {
        let mut one = view(3);
        assert_eq!(
            one.type_char('S'),
            Typed::Landed(JumpTarget::Tab(ProjectId("p1".into())))
        );
        assert_eq!(view(3).type_char('z'), Typed::Missed);

        let mut two = view(30);
        assert_eq!(two.type_char('a'), Typed::Pending);
        assert_eq!(
            two.type_char('s'),
            Typed::Landed(JumpTarget::Tab(ProjectId("p1".into())))
        );
        let mut stray = view(30);
        assert_eq!(stray.type_char('a'), Typed::Pending);
        assert_eq!(stray.type_char('1'), Typed::Missed);
    }
}
