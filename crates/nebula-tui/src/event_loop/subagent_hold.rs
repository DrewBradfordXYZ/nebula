//! The SUBAGENT HOLD: a session about to be started in, or moved into, a
//! Claude Code SUBAGENT CHECKOUT (`Worktree::claude_subagent`) waits for a
//! yes first. Claude Code removes that checkout when the subagent finishes,
//! and a session there is stranded with it.
//!
//! It works on the requests an input event put out rather than on the keys
//! that made them: a quick prompt, a preset, a PR session, `m` and a card
//! dragged onto a band all end in one `CreateAgent` or `MoveAgent`, so
//! every way in is covered by one check. The request is taken out of the
//! batch whole, its `req_id` and intent already registered, and sent as it
//! was on a yes. Any other way out of the dialog cancels it as the DAEMON's
//! refusal would, through the same error path: the stand-in card comes
//! down, a move is put back, and a quick prompt reopens with its text.

use super::*;

/// What the hold is guarding against: the start (or move) and the
/// checkout's branch.
fn held_target(app: &App, req: &ClientRequest) -> Option<(bool, String, String)> {
    let (moving, worktree) = match req {
        ClientRequest::CreateAgent { worktree, .. } => (false, worktree),
        ClientRequest::MoveAgent { worktree, .. } => (true, worktree),
        _ => return None,
    };
    let w = app.tree.worktrees.iter().find(|w| &w.id == worktree)?;
    let subagent = w.claude_subagent()?.to_string();
    Some((moving, w.branch.clone(), subagent))
}

fn req_id_of(req: &ClientRequest) -> Option<u64> {
    match req {
        ClientRequest::CreateAgent { req_id, .. } | ClientRequest::MoveAgent { req_id, .. } => {
            Some(*req_id)
        }
        _ => None,
    }
}

/// After an input event: a hold whose dialog went without a yes is
/// cancelled, then a start or move this event aimed at a subagent checkout
/// is held behind a new one. A request let through by a yes is never held
/// again.
pub(crate) fn hold_for_subagent_checkout(app: &mut App, out: &mut Vec<ClientRequest>) {
    let asking = matches!(
        &app.modals.overlay,
        Some(Overlay::Confirm(c)) if c.action == PendingAction::SendSubagentHold
    );
    if !asking {
        if let Some(held) = app.requests.subagent_hold.take() {
            cancel(app, &held, out);
        }
    }
    if app.requests.subagent_hold.is_some() {
        return;
    }
    let allowed = app.requests.subagent_allowed.take();
    let Some(at) = out
        .iter()
        .position(|req| req_id_of(req) != allowed && held_target(app, req).is_some())
    else {
        return;
    };
    let req = out.remove(at);
    let Some((moving, branch, subagent)) = held_target(app, &req) else {
        return;
    };
    let (verb, question) = if moving {
        ("moved", "Move it there anyway?")
    } else {
        ("started", "Start it there anyway?")
    };
    app.modals.overlay = Some(Overlay::Confirm(ConfirmDialog {
        title: "Subagent checkout".into(),
        message: format!(
            "{branch} is a Claude Code subagent's checkout ({subagent}). Claude Code removes \
             it when the subagent finishes, and a session {verb} there stops with it. {question}"
        ),
        action: PendingAction::SendSubagentHold,
        area: ratatui::layout::Rect::default(),
    }));
    app.requests.subagent_hold = Some(req);
    app.chrome.dirty = true;
}

/// The dialog's yes: the held request goes out as it was built.
pub(crate) fn send_subagent_hold(app: &mut App, out: &mut Vec<ClientRequest>) {
    if let Some(req) = app.requests.subagent_hold.take() {
        app.requests.subagent_allowed = req_id_of(&req);
        out.push(req);
    }
}

/// Anything but a yes: unwound as the DAEMON's refusal of it would be.
fn cancel(app: &mut App, held: &ClientRequest, out: &mut Vec<ClientRequest>) {
    let message = match held_target(app, held) {
        Some((true, branch, _)) => {
            format!("not moved: {branch} is a Claude Code subagent's checkout")
        }
        Some((false, branch, _)) => {
            format!("not started: {branch} is a Claude Code subagent's checkout")
        }
        // The row went while the dialog was up: still unwound, so no
        // stand-in or intent outlives it.
        None => "not sent: the subagent's checkout is gone".into(),
    };
    handle_server_event(
        app,
        ServerEvent::Error {
            req_id: req_id_of(held),
            message,
        },
        out,
    );
}
