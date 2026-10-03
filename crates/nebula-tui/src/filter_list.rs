//! Shared fuzzy-list state for modal overlays.
//!
//! A modal still owns what choosing a row means, plus its draw/key/mouse
//! entry points. This type owns the repeatable mechanics: the query, the
//! filtered rows, the cursor, the visible window and row hit-testing.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position, Rect};

use crate::text_input::TextInput;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterMatch {
    /// Index into [`FilterList::items`].
    pub item: usize,
    /// Character positions in the item's label matched by the query.
    pub positions: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterList<T> {
    pub items: Vec<T>,
    pub query: TextInput,
    pub matches: Vec<FilterMatch>,
    /// Cursor into `matches`.
    pub cursor: usize,
    /// First visible row as of the last draw.
    pub scroll: usize,
    /// Screen rect of the result rows, written during draw for hit-testing.
    pub list_area: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterListKey {
    None,
    Close,
    Cleared,
    Moved,
    QueryChanged,
}

impl<T> Default for FilterList<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            query: TextInput::new(),
            matches: Vec::new(),
            cursor: 0,
            scroll: 0,
            list_area: Rect::default(),
        }
    }
}

impl<T> FilterList<T> {
    pub fn new(items: Vec<T>) -> Self {
        Self {
            items,
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.matches.len()
    }

    pub fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub fn query_has_words(&self) -> bool {
        self.query.split_whitespace().next().is_some()
    }

    pub fn set_items(&mut self, items: Vec<T>) {
        self.items = items;
        self.cursor = self.cursor.min(self.matches.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&T> {
        let m = self.matches.get(self.cursor)?;
        self.items.get(m.item)
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.matches.get(self.cursor).map(|m| m.item)
    }

    pub fn select(&mut self, index: i64) -> bool {
        let next = clamp_selection(index, self.matches.len());
        let changed = next != self.cursor;
        self.cursor = next;
        changed
    }

    pub fn step(&mut self, delta: i64) -> bool {
        self.select(self.cursor as i64 + delta)
    }

    pub fn move_to_item(&mut self, item: usize) -> bool {
        if let Some(row) = self.matches.iter().position(|m| m.item == item) {
            let changed = self.cursor != row;
            self.cursor = row;
            changed
        } else {
            false
        }
    }

    pub fn window_start(&self, height: usize) -> usize {
        window_start(self.cursor, height)
    }

    pub fn sync_scroll(&mut self, height: usize) -> usize {
        self.scroll = self.window_start(height);
        self.scroll
    }

    pub fn hit(&self, pos: Position) -> Option<usize> {
        crate::list_hit::row_at(self.list_area, self.scroll, self.matches.len(), pos)
    }

    pub fn hit_item(&self, pos: Position) -> Option<usize> {
        let row = self.hit(pos)?;
        self.matches.get(row).map(|m| m.item)
    }

    pub fn replace_matches(&mut self, matches: Vec<FilterMatch>) {
        self.matches = matches;
        self.cursor = clamp_selection(self.cursor as i64, self.matches.len());
        self.scroll = self.scroll.min(self.matches.len().saturating_sub(1));
    }

    pub fn apply_filter_by(
        &mut self,
        labels: impl IntoIterator<Item = String>,
        rank: impl Fn(usize) -> usize,
    ) {
        let labels: Vec<String> = labels.into_iter().collect();
        let matches = crate::fuzzy::rank_by(
            self.query.as_str(),
            labels.iter().map(String::as_str),
            |i, _| rank(i),
        )
        .into_iter()
        .map(|(item, positions)| FilterMatch { item, positions })
        .collect();
        self.replace_matches(matches);
    }

    pub fn apply_filter(&mut self, label: impl Fn(&T) -> String) {
        let labels: Vec<String> = self.items.iter().map(label).collect();
        self.apply_filter_by(labels, |i| i);
    }

    /// [`apply_filter`](Self::apply_filter) for a list whose query may also
    /// carry `key:value` terms ([`QueryTerms`]) naming the `keys` given: the
    /// terms are lifted out and handed to `keeps`, which drops the rows they
    /// rule out, and only the text left over ranks and lights `label`.
    pub fn apply_filter_where(
        &mut self,
        keys: &[&'static str],
        label: impl Fn(&T) -> String,
        keeps: impl Fn(&T, &QueryTerms) -> bool,
    ) {
        let matches = rank_where(self.query.as_str(), keys, &self.items, label, keeps);
        self.replace_matches(matches);
    }

    /// Apply the common modal keys: two-stage Esc, Up/Down, Ctrl+n/p and
    /// text input. PageUp/PageDown are included because the branch switcher
    /// and long file lists have always treated them as larger cursor moves.
    pub fn handle_standard_key(&mut self, key: &KeyEvent, page: i64) -> FilterListKey {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                FilterListKey::Cleared
            }
            KeyCode::Esc => FilterListKey::Close,
            KeyCode::Down => {
                self.step(1);
                FilterListKey::Moved
            }
            KeyCode::Up => {
                self.step(-1);
                FilterListKey::Moved
            }
            KeyCode::Char('n') if ctrl => {
                self.step(1);
                FilterListKey::Moved
            }
            KeyCode::Char('p') if ctrl => {
                self.step(-1);
                FilterListKey::Moved
            }
            KeyCode::PageDown => {
                self.step(page.max(1));
                FilterListKey::Moved
            }
            KeyCode::PageUp => {
                self.step(-page.max(1));
                FilterListKey::Moved
            }
            _ => {
                if self.query.handle_key(key).changed() {
                    FilterListKey::QueryChanged
                } else {
                    FilterListKey::None
                }
            }
        }
    }
}

/// The rows of `items` that `query` leaves, best first: its `key:value`
/// terms for `keys` filter through `keeps`, and the text left over
/// fuzzy-ranks and lights `label`. What
/// [`FilterList::apply_filter_where`] runs, for a caller that needs the
/// answer for a query other than the typed one.
pub fn rank_where<T>(
    query: &str,
    keys: &[&'static str],
    items: &[T],
    label: impl Fn(&T) -> String,
    keeps: impl Fn(&T, &QueryTerms) -> bool,
) -> Vec<FilterMatch> {
    let terms = QueryTerms::parse(query, keys);
    let labels: Vec<String> = items.iter().map(label).collect();
    // `apply_filter`'s ranking, ties kept in list order, over the rows the
    // terms leave.
    crate::fuzzy::rank_by(&terms.text, labels.iter().map(String::as_str), |i, _| i)
        .into_iter()
        .filter(|(item, _)| keeps(&items[*item], &terms))
        .map(|(item, positions)| FilterMatch { item, positions })
        .collect()
}

/// A modal query split into the `key:value` terms that modal knows and the
/// free text left for fuzzy ranking: with keys `["label"]`, `label:bug
/// login` is one `label` term (`bug`) and the text `login`. A value is the
/// word after the colon, or a quoted phrase for one with spaces
/// (`label:"good first issue"`; the closing quote may still be untyped).
/// Keys are case-insensitive; values keep the case typed, and a modal
/// compares them as it likes. A bare `key:` — the moment before its value
/// is typed — is dropped rather than read as text, so the list doesn't
/// blink empty mid-word. A word that merely contains a key (`relabel:x`)
/// stays text. There is no negation: `-label:x` is text.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct QueryTerms {
    pub terms: Vec<QueryTerm>,
    pub text: String,
}

/// One `key:value` term of a [`QueryTerms`], with where it sits in the
/// query — key, value and any quotes — so a modal can take it back out or
/// replace it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryTerm {
    pub key: &'static str,
    pub value: String,
    pub span: std::ops::Range<usize>,
}

impl QueryTerms {
    pub fn parse(query: &str, keys: &[&'static str]) -> Self {
        let mut out = QueryTerms::default();
        let mut text: Vec<&str> = Vec::new();
        let mut rest = query.trim_start();
        while !rest.is_empty() {
            let start = query.len() - rest.len();
            let key = keys.iter().copied().find(|key| {
                rest.get(..key.len())
                    .is_some_and(|head| head.eq_ignore_ascii_case(key))
                    && rest[key.len()..].starts_with(':')
            });
            let after = match key {
                Some(key) => {
                    let value = &rest[key.len() + 1..];
                    let (term, after) = match value.strip_prefix('"') {
                        Some(quoted) => match quoted.find('"') {
                            Some(end) => (&quoted[..end], &quoted[end + 1..]),
                            None => (quoted, ""),
                        },
                        None => {
                            let end = value.find(char::is_whitespace).unwrap_or(value.len());
                            (&value[..end], &value[end..])
                        }
                    };
                    let term = term.trim();
                    if !term.is_empty() {
                        out.terms.push(QueryTerm {
                            key,
                            value: term.to_string(),
                            span: start..query.len() - after.len(),
                        });
                    }
                    after
                }
                None => {
                    let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                    text.push(&rest[..end]);
                    &rest[end..]
                }
            };
            rest = after.trim_start();
        }
        out.text = text.join(" ");
        out
    }

    /// The values typed for `key`, in order, as typed.
    pub fn values<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.terms
            .iter()
            .filter(move |term| term.key == key)
            .map(|term| term.value.as_str())
    }
}

pub fn clamp_selection(index: i64, len: usize) -> usize {
    let max = len.saturating_sub(1) as i64;
    index.clamp(0, max) as usize
}

pub fn window_start(selected: usize, height: usize) -> usize {
    (selected + 1).saturating_sub(height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(query: &str) -> QueryTerms {
        QueryTerms::parse(query, &["label"])
    }

    fn split(query: &str) -> (Vec<(&'static str, String)>, String) {
        let parsed = terms(query);
        (
            parsed.terms.into_iter().map(|t| (t.key, t.value)).collect(),
            parsed.text,
        )
    }

    #[test]
    fn known_keys_split_from_the_text() {
        assert_eq!(terms(""), QueryTerms::default());
        assert_eq!(
            split("login label:bug redirect"),
            (vec![("label", "bug".into())], "login redirect".into())
        );
        assert_eq!(
            split("LABEL:UI label:\"good first issue\" crash"),
            (
                vec![("label", "UI".into()), ("label", "good first issue".into())],
                "crash".into()
            )
        );
        // Mid-typing: a bare key filters nothing, an open quote takes the
        // rest of the line.
        assert_eq!(terms("label:"), QueryTerms::default());
        assert_eq!(
            split("label:\"good fir"),
            (vec![("label", "good fir".into())], String::new())
        );
        // Only a leading key is a term, and an unknown key is text; so is a
        // negated one, there being no negation.
        assert!(terms("relabel:x").terms.is_empty());
        assert!(terms("xlabel:\"x y\"").terms.is_empty());
        assert_eq!(terms("author:me").text, "author:me");
        assert_eq!(terms("-label:bug").text, "-label:bug");
        // A multi-byte character where the colon would sit is plain text.
        assert_eq!(terms("labelé").text, "labelé");
        assert_eq!(
            terms("label:a label:b").values("label").collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    /// Each term knows where it sits, quotes included, so a modal can
    /// take it back out.
    #[test]
    fn terms_carry_their_place_in_the_query() {
        let query = "  login label:\"good first\" x label:bug";
        let parsed = terms(query);
        assert_eq!(&query[parsed.terms[0].span.clone()], "label:\"good first\"");
        assert_eq!(parsed.terms[0].span, 8..26);
        assert_eq!(&query[parsed.terms[1].span.clone()], "label:bug");
        assert_eq!(parsed.text, "login x");
        let open = "a label:\"good fi";
        assert_eq!(terms(open).terms[0].span, 2..open.len());
    }

    /// Terms or not, the text ranks as `apply_filter` ranks it: matches
    /// that score the same keep the list's own order, never shortest
    /// first.
    #[test]
    fn terms_keep_apply_filters_ranking() {
        // Both start with the word, so both score the same; the shorter
        // one is second in the list and must stay there.
        let items = vec!["login fails on linux".to_string(), "login slow".to_string()];
        let order = |where_: bool| -> Vec<usize> {
            let mut list = FilterList::new(items.clone());
            list.query.set_text("login");
            if where_ {
                list.apply_filter_where(&["label"], |s| s.clone(), |_, _| true);
            } else {
                list.apply_filter(|s| s.clone());
            }
            list.matches.iter().map(|m| m.item).collect()
        };
        assert_eq!(order(false), [0, 1], "apply_filter keeps list order");
        assert_eq!(order(true), [0, 1]);
    }
}
