//! Chromium session files ("SNSS"): what a browser's own session restore
//! knows about its windows and tabs. Chrome appends a command to
//! `<profile>/Sessions/Session_<n>` a couple of seconds after anything
//! changes, so the file is the same picture the browser would rebuild after
//! a crash: every tab's URL, title, pin state, and when it was last the
//! active tab. Nothing here writes, and no browser is asked anything.
//!
//! The format is an 8-byte header (`SNSS`, then a version) followed by
//! records of `u16 size, u8 command, payload`. Commands we do not know are
//! skipped by size, which is how the reader survives new Chrome versions.

use std::collections::BTreeMap;

/// Chrome stamps times in microseconds since 1601-01-01.
const WINDOWS_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;

const CMD_SET_TAB_WINDOW: u8 = 0;
const CMD_SET_TAB_INDEX_IN_WINDOW: u8 = 2;
const CMD_NAV_PRUNED_FROM_BACK: u8 = 5;
const CMD_UPDATE_TAB_NAVIGATION: u8 = 6;
const CMD_SET_SELECTED_NAVIGATION_INDEX: u8 = 7;
const CMD_SET_SELECTED_TAB_IN_INDEX: u8 = 8;
const CMD_SET_WINDOW_TYPE: u8 = 9;
const CMD_NAV_PRUNED_FROM_FRONT: u8 = 11;
const CMD_SET_PINNED_STATE: u8 = 12;
const CMD_TAB_CLOSED: u8 = 16;
const CMD_WINDOW_CLOSED: u8 = 17;
const CMD_LAST_ACTIVE_TIME: u8 = 21;
const CMD_NAV_PRUNED: u8 = 24;

/// Window types, from `sessions::SessionWindow::WindowType`.
const WINDOW_DEVTOOLS: i32 = 3;

/// A tab as the session file describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    /// The tab's session id. The same number the browser's AppleScript
    /// dictionary reports as the tab `id`, which is what makes closing by id
    /// possible.
    pub id: i32,
    pub window_id: i32,
    /// Position in its window.
    pub index: i32,
    pub pinned: bool,
    /// True for the tab currently selected in its window.
    pub active: bool,
    pub url: String,
    pub title: String,
    /// Epoch seconds when the tab was last the active one, if recorded.
    pub last_active: Option<u64>,
}

#[derive(Debug, Default)]
struct WindowState {
    kind: i32,
    selected_index: i32,
    closed: bool,
}

#[derive(Debug, Default)]
struct TabState {
    window_id: Option<i32>,
    index: i32,
    pinned: bool,
    /// Navigation entries by index: (url, title).
    navs: BTreeMap<i32, (String, String)>,
    selected_nav: Option<i32>,
    last_active: Option<i64>,
    closed: bool,
}

#[derive(Debug, Default)]
pub struct Session {
    pub version: i32,
    windows: BTreeMap<i32, WindowState>,
    tabs: BTreeMap<i32, TabState>,
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn i64_at(b: &[u8], at: usize) -> Option<i64> {
    Some(i64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// A cursor over a `base::Pickle` payload: 4-byte aligned fields.
struct Pickle<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Pickle<'a> {
    /// The command payload starts with the pickle's own header, a u32
    /// payload size. Skip it.
    fn new(payload: &'a [u8]) -> Option<Self> {
        if payload.len() < 4 {
            return None;
        }
        Some(Pickle { b: payload, p: 4 })
    }

    fn int(&mut self) -> Option<i32> {
        let v = i32_at(self.b, self.p)?;
        self.p += 4;
        Some(v)
    }

    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.p..self.p + n)?;
        self.p += (n + 3) & !3;
        Some(s)
    }

    fn string(&mut self) -> Option<String> {
        let n = usize::try_from(self.int()?).ok()?;
        Some(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }

    fn string16(&mut self) -> Option<String> {
        let n = usize::try_from(self.int()?).ok()?;
        let raw = self.bytes(n.checked_mul(2)?)?;
        let units: Vec<u16> = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        Some(String::from_utf16_lossy(&units))
    }
}

impl Session {
    fn tab(&mut self, id: i32) -> &mut TabState {
        self.tabs.entry(id).or_default()
    }

    fn window(&mut self, id: i32) -> &mut WindowState {
        self.windows.entry(id).or_default()
    }

    fn apply(&mut self, cmd: u8, payload: &[u8]) {
        match cmd {
            CMD_SET_TAB_WINDOW => {
                if let (Some(w), Some(t)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    self.tab(t).window_id = Some(w);
                    self.window(w);
                }
            }
            CMD_SET_TAB_INDEX_IN_WINDOW => {
                if let (Some(t), Some(i)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    self.tab(t).index = i;
                }
            }
            CMD_UPDATE_TAB_NAVIGATION => {
                let parsed = Pickle::new(payload).and_then(|mut p| {
                    let t = p.int()?;
                    let i = p.int()?;
                    let url = p.string()?;
                    let title = p.string16()?;
                    Some((t, i, url, title))
                });
                if let Some((t, i, url, title)) = parsed {
                    self.tab(t).navs.insert(i, (url, title));
                }
            }
            CMD_SET_SELECTED_NAVIGATION_INDEX => {
                if let (Some(t), Some(i)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    self.tab(t).selected_nav = Some(i);
                }
            }
            CMD_SET_SELECTED_TAB_IN_INDEX => {
                if let (Some(w), Some(i)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    self.window(w).selected_index = i;
                }
            }
            CMD_SET_WINDOW_TYPE => {
                if let (Some(w), Some(k)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    self.window(w).kind = k;
                }
            }
            CMD_SET_PINNED_STATE => {
                if let (Some(t), Some(&p)) = (i32_at(payload, 0), payload.get(4)) {
                    self.tab(t).pinned = p != 0;
                }
            }
            CMD_TAB_CLOSED => {
                if let Some(t) = i32_at(payload, 0) {
                    self.tab(t).closed = true;
                }
            }
            CMD_WINDOW_CLOSED => {
                if let Some(w) = i32_at(payload, 0) {
                    self.window(w).closed = true;
                }
            }
            CMD_LAST_ACTIVE_TIME => {
                // `struct { int32 tab_id; int64 time; }` copied with its padding.
                if let (Some(t), Some(us)) = (i32_at(payload, 0), i64_at(payload, 8)) {
                    self.tab(t).last_active = Some(us);
                }
            }
            CMD_NAV_PRUNED_FROM_BACK => {
                if let (Some(t), Some(i)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    self.tab(t).navs.retain(|k, _| *k < i);
                }
            }
            CMD_NAV_PRUNED_FROM_FRONT => {
                if let (Some(t), Some(n)) = (i32_at(payload, 0), i32_at(payload, 4)) {
                    let tab = self.tab(t);
                    tab.navs = std::mem::take(&mut tab.navs)
                        .into_iter()
                        .filter(|(k, _)| *k >= n)
                        .map(|(k, v)| (k - n, v))
                        .collect();
                }
            }
            CMD_NAV_PRUNED => {
                if let (Some(t), Some(i), Some(n)) =
                    (i32_at(payload, 0), i32_at(payload, 4), i32_at(payload, 8))
                {
                    let tab = self.tab(t);
                    tab.navs = std::mem::take(&mut tab.navs)
                        .into_iter()
                        .filter(|(k, _)| *k < i || *k >= i + n)
                        .map(|(k, v)| if k >= i + n { (k - n, v) } else { (k, v) })
                        .collect();
                }
            }
            _ => {}
        }
    }

    /// Tabs that are still open, in window order then tab order.
    pub fn live_tabs(&self) -> Vec<Tab> {
        let mut out: Vec<Tab> = self
            .tabs
            .iter()
            .filter(|(_, t)| !t.closed)
            .filter_map(|(id, t)| {
                let w = t.window_id?;
                let win = self.windows.get(&w)?;
                if win.closed || win.kind == WINDOW_DEVTOOLS {
                    return None;
                }
                let nav = t
                    .selected_nav
                    .and_then(|i| t.navs.get(&i))
                    .or_else(|| t.navs.values().next_back())?;
                let last_active = t
                    .last_active
                    .filter(|us| *us > 0)
                    .map(|us| us / 1_000_000 - WINDOWS_EPOCH_OFFSET_SECS)
                    .and_then(|s| u64::try_from(s).ok());
                Some(Tab {
                    id: *id,
                    window_id: w,
                    index: t.index,
                    pinned: t.pinned,
                    active: false,
                    url: nav.0.clone(),
                    title: nav.1.clone(),
                    last_active,
                })
            })
            .collect();
        out.sort_by_key(|t| (t.window_id, t.index, t.id));
        // The selected index counts the window's open tabs in visual order.
        let mut pos_in_window: BTreeMap<i32, i32> = BTreeMap::new();
        for t in &mut out {
            let pos = pos_in_window.entry(t.window_id).or_insert(0);
            let selected = self
                .windows
                .get(&t.window_id)
                .map(|w| w.selected_index)
                .unwrap_or(-1);
            t.active = *pos == selected;
            *pos += 1;
        }
        out
    }
}

/// Parse a session file. None when the header is not SNSS.
pub fn parse(bytes: &[u8]) -> Option<Session> {
    if bytes.len() < 8 || &bytes[..4] != b"SNSS" {
        return None;
    }
    let mut s = Session {
        version: i32_at(bytes, 4)?,
        ..Default::default()
    };
    let mut off = 8;
    while off + 3 <= bytes.len() {
        let size = u16::from_le_bytes([bytes[off], bytes[off + 1]]) as usize;
        if size == 0 {
            break;
        }
        let cmd = bytes[off + 2];
        let end = off + 2 + size;
        let Some(payload) = bytes.get(off + 3..end) else {
            break; // truncated tail: Chrome was mid-write
        };
        s.apply(cmd, payload);
        off = end;
    }
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(cmd: u8, payload: &[u8]) -> Vec<u8> {
        let mut v = ((payload.len() + 1) as u16).to_le_bytes().to_vec();
        v.push(cmd);
        v.extend_from_slice(payload);
        v
    }

    fn pickle_nav(tab: i32, index: i32, url: &str, title: &str) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&tab.to_le_bytes());
        body.extend_from_slice(&index.to_le_bytes());
        body.extend_from_slice(&(url.len() as i32).to_le_bytes());
        body.extend_from_slice(url.as_bytes());
        while body.len() % 4 != 0 {
            body.push(0);
        }
        let units: Vec<u16> = title.encode_utf16().collect();
        body.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for u in units {
            body.extend_from_slice(&u.to_le_bytes());
        }
        while body.len() % 4 != 0 {
            body.push(0);
        }
        let mut payload = (body.len() as u32).to_le_bytes().to_vec();
        payload.extend_from_slice(&body);
        raw(CMD_UPDATE_TAB_NAVIGATION, &payload)
    }

    fn ints(v: &[i32]) -> Vec<u8> {
        v.iter().flat_map(|i| i.to_le_bytes()).collect()
    }

    #[test]
    fn rebuilds_open_tabs() {
        let mut f = b"SNSS".to_vec();
        f.extend_from_slice(&3i32.to_le_bytes());
        f.extend(raw(CMD_SET_WINDOW_TYPE, &ints(&[10, 0])));
        f.extend(raw(CMD_SET_TAB_WINDOW, &ints(&[10, 1])));
        f.extend(raw(CMD_SET_TAB_INDEX_IN_WINDOW, &ints(&[1, 1])));
        f.extend(raw(CMD_SET_TAB_WINDOW, &ints(&[10, 2])));
        f.extend(raw(CMD_SET_TAB_INDEX_IN_WINDOW, &ints(&[2, 0])));
        f.extend(pickle_nav(1, 0, "https://a.example/", "A first"));
        f.extend(pickle_nav(1, 1, "https://a.example/two", "A – second ✓"));
        f.extend(raw(CMD_SET_SELECTED_NAVIGATION_INDEX, &ints(&[1, 1])));
        f.extend(pickle_nav(2, 0, "https://b.example/", "B"));
        f.extend(raw(CMD_SET_PINNED_STATE, &[2, 0, 0, 0, 1, 0, 0, 0]));
        // last active: 2001-09-09T01:46:40Z = 1e9 epoch seconds
        let mut la = ints(&[2, 0]);
        la.extend_from_slice(
            &((1_000_000_000 + WINDOWS_EPOCH_OFFSET_SECS) * 1_000_000).to_le_bytes(),
        );
        f.extend(raw(CMD_LAST_ACTIVE_TIME, &la));
        f.extend(raw(CMD_SET_SELECTED_TAB_IN_INDEX, &ints(&[10, 1])));
        // A third tab that was closed, and a devtools window with a tab.
        f.extend(raw(CMD_SET_TAB_WINDOW, &ints(&[10, 3])));
        f.extend(pickle_nav(3, 0, "https://gone.example/", "gone"));
        f.extend(raw(CMD_TAB_CLOSED, &ints(&[3, 0, 0, 0])));
        f.extend(raw(CMD_SET_WINDOW_TYPE, &ints(&[11, WINDOW_DEVTOOLS])));
        f.extend(raw(CMD_SET_TAB_WINDOW, &ints(&[11, 4])));
        f.extend(pickle_nav(4, 0, "devtools://devtools/", "DevTools"));
        // An unknown command is skipped by size.
        f.extend(raw(200, &[9; 13]));
        f.extend(raw(255, &[]));

        let s = parse(&f).unwrap();
        assert_eq!(s.version, 3);
        let tabs = s.live_tabs();
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].id, 2);
        assert_eq!(tabs[0].url, "https://b.example/");
        assert!(tabs[0].pinned);
        assert!(!tabs[0].active);
        assert_eq!(tabs[0].last_active, Some(1_000_000_000));
        assert_eq!(tabs[1].id, 1);
        assert_eq!(tabs[1].url, "https://a.example/two");
        assert_eq!(tabs[1].title, "A – second ✓");
        assert!(tabs[1].active);
        assert_eq!(tabs[1].last_active, None);
    }

    #[test]
    fn prunes_navigation_history() {
        let mut f = b"SNSS".to_vec();
        f.extend_from_slice(&3i32.to_le_bytes());
        f.extend(raw(CMD_SET_TAB_WINDOW, &ints(&[10, 1])));
        for i in 0..4 {
            f.extend(pickle_nav(1, i, &format!("https://x/{i}"), ""));
        }
        f.extend(raw(CMD_SET_SELECTED_NAVIGATION_INDEX, &ints(&[1, 3])));
        // Drop the first two: what was 3 is now 1.
        f.extend(raw(CMD_NAV_PRUNED_FROM_FRONT, &ints(&[1, 2])));
        f.extend(raw(CMD_SET_SELECTED_NAVIGATION_INDEX, &ints(&[1, 1])));
        let tabs = parse(&f).unwrap().live_tabs();
        assert_eq!(tabs[0].url, "https://x/3");
    }

    #[test]
    fn rejects_other_files() {
        assert!(parse(b"not a session file at all").is_none());
        assert!(parse(b"SNSS").is_none());
    }

    #[test]
    fn tolerates_a_truncated_tail() {
        let mut f = b"SNSS".to_vec();
        f.extend_from_slice(&3i32.to_le_bytes());
        f.extend(raw(CMD_SET_TAB_WINDOW, &ints(&[10, 1])));
        f.extend(pickle_nav(1, 0, "https://ok/", "ok"));
        f.extend_from_slice(&[0x40, 0x00, 0x06, 1, 2, 3]); // claims 64 bytes, has 3
        let tabs = parse(&f).unwrap().live_tabs();
        assert_eq!(tabs.len(), 1);
    }
}
