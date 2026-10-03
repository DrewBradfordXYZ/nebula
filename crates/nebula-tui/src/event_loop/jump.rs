//! JUMP MODE's keys (`crate::jump` is its model and drawing).
//!
//! A label on a card is a jump, and lands as nebula's other jumps do —
//! `/` `Enter` and `.` / `,`: **Search Enter attaches**
//! (`palette_enter_attaches`) decides whether the session opens or the
//! cursor only lands on it, and a session that NEEDS FEEDBACK opens either
//! way (`Landing::for_enter_on`). A session card goes through the very
//! `jump_to_target` those two run; a terminal card, which the palette
//! never lists, lands as the walk does and opens as Enter on it does.
//!
//! The rest have nothing to open: a band through the same
//! [`launcher::select_band_of`] a click on its rule runs, a tab through
//! the same [`launcher::click_tab`] a click on it runs.

use crossterm::event::{KeyCode, KeyEvent};
use nebula_core::ClientRequest;

use super::{jump_to_target, launcher, Landing};
use crate::app::{App, Overlay};
use crate::jump::{JumpTarget, JumpView, Typed};
use crate::palette::PaletteTarget;
use nebula_core::SessionRef;

/// What the footer says while the labels are up.
const HINT: &str = "jump: type a label · Esc cancels";

/// The jump key on the grid: label what the last frame drew.
pub(super) fn open(app: &mut App) {
    match JumpView::from_screen(app) {
        Some(view) => {
            app.overlay = Some(Overlay::Jump(view));
            app.flash = Some(HINT.into());
        }
        None => app.flash = Some("nothing on screen to jump to".into()),
    }
    app.dirty = true;
}

/// A key while the labels are up. Every key is the mode's: a letter types
/// at a label, Backspace takes one back, and anything else — Esc, or a key
/// no label starts with — closes the mode where the cursor already was.
pub(super) fn handle_key(app: &mut App, key: KeyEvent, out: &mut Vec<ClientRequest>) {
    let Some(Overlay::Jump(view)) = &mut app.overlay else {
        return;
    };
    app.dirty = true;
    let typed = match key.code {
        KeyCode::Backspace => {
            view.typed.pop();
            return;
        }
        KeyCode::Char(c) => view.type_char(c),
        _ => {
            close(app);
            return;
        }
    };
    match typed {
        Typed::Pending => {}
        Typed::Missed => {
            close(app);
            app.flash = Some("no label there".into());
        }
        Typed::Landed(target) => {
            close(app);
            land(app, target, out);
        }
    }
}

fn close(app: &mut App) {
    app.overlay = None;
    if app.flash.as_deref() == Some(HINT) {
        app.flash = None;
    }
}

fn land(app: &mut App, target: JumpTarget, out: &mut Vec<ClientRequest>) {
    match target {
        JumpTarget::Card(SessionRef::Agent(id)) => {
            let attaches = crate::config::Config::load().palette_enter_attaches;
            let target = PaletteTarget::Session(id);
            let landing = Landing::for_enter_on(app, &target, attaches);
            jump_to_target(app, target, landing, out);
        }
        JumpTarget::Card(sref) => {
            launcher::select_card(app, sref, out);
            if crate::config::Config::load().palette_enter_attaches {
                launcher::enter_pane(app, out);
            }
        }
        JumpTarget::Band(worktree) => {
            launcher::select_band_of(app, &worktree, out);
        }
        JumpTarget::Tab(id) => launcher::click_tab(app, &id, out),
    }
}
