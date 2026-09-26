//! Browser state: servers → shares → folders → videos. Talks to the
//! [`Library`] workers and turns their results into a [`View`].

use super::browser::{Dialog, Icon, Row, View, format_size};
use crate::library::{Library, Opened, Path, Request, Response};
use crate::playability::{Assessment, Verdict};

const FOOTER: &str = "Trigger: open  ·  B: back  ·  Stick: scroll";
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "m4v", "mkv", "mov", "webm", "avi", "ts", "m2ts"];

pub fn is_video(name: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

#[derive(Clone, Debug, PartialEq)]
enum Location {
    Servers,
    Shares {
        server: usize,
    },
    Folder {
        server: usize,
        share: String,
        path: Path,
    },
}

enum Item {
    Server(usize),
    Share(String),
    Dir(String),
    Video {
        name: String,
        size: u64,
        assessment: Option<Assessment>,
        broken: Option<String>,
    },
}

pub struct Navigator {
    library: Library,
    location: Location,
    items: Vec<Item>,
    view: View,
    pending: Option<u64>,
    next_id: u64,
    generation: u64,
    dirty: bool,
}

impl Navigator {
    pub fn new(library: Library) -> Self {
        let mut nav = Self {
            library,
            location: Location::Servers,
            items: Vec::new(),
            view: View::default(),
            pending: None,
            next_id: 1,
            generation: 0,
            dirty: true,
        };
        nav.show_servers();
        nav
    }

    pub fn view(&self) -> &View {
        &self.view
    }

    /// True (once) when the view changed and must be redrawn.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn scroll_by(&mut self, rows: f32) {
        let before = self.view.scroll;
        self.view.scroll += rows;
        self.view.clamp_scroll();
        if self.view.scroll != before {
            self.dirty = true;
        }
    }

    fn request_id(&mut self) -> u64 {
        self.next_id += 1;
        self.pending = Some(self.next_id);
        self.next_id
    }

    fn title(&self) -> String {
        let name = |server: usize| {
            self.library
                .servers
                .get(server)
                .map_or_else(|| "?".into(), |s| s.name.clone())
        };
        match &self.location {
            Location::Servers => "Just Video".into(),
            Location::Shares { server } => name(*server),
            Location::Folder {
                server,
                share,
                path,
            } => {
                let mut parts = vec![name(*server), share.clone()];
                parts.extend(path.iter().cloned());
                parts.join("  ›  ")
            }
        }
    }

    fn set_status(&mut self, status: impl Into<String>) {
        self.view.status = Some(status.into());
        self.view.rows.clear();
        self.dirty = true;
    }

    fn show_servers(&mut self) {
        self.location = Location::Servers;
        self.pending = None;
        self.items = (0..self.library.servers.len()).map(Item::Server).collect();
        self.view = View {
            title: self.title(),
            footer: FOOTER.into(),
            ..Default::default()
        };
        if self.items.is_empty() {
            self.set_status(
                "No servers saved yet. On your PC, run:  just-video add-server smb://user@host  (with the app installed on the headset), then open Just Video again.",
            );
        }
        self.rebuild_rows();
    }

    fn navigate(&mut self, location: Location) {
        self.location = location.clone();
        self.items.clear();
        self.generation += 1;
        self.library.set_probe_generation(self.generation);
        self.view = View {
            title: self.title(),
            footer: FOOTER.into(),
            ..Default::default()
        };
        let id = self.request_id();
        match location {
            Location::Servers => self.show_servers(),
            Location::Shares { server } => {
                self.set_status("Connecting…");
                self.library.send(Request::Shares { id, server });
            }
            Location::Folder {
                server,
                share,
                path,
            } => {
                self.set_status("Loading…");
                self.library.send(Request::List {
                    id,
                    server,
                    share,
                    path,
                });
            }
        }
    }

    fn rebuild_rows(&mut self) {
        let servers = &self.library.servers;
        self.view.rows = self
            .items
            .iter()
            .map(|item| match item {
                Item::Server(i) => Row {
                    icon: Icon::Server,
                    label: servers[*i].name.clone(),
                    detail: servers[*i].url.clone(),
                    right: String::new(),
                },
                Item::Share(name) => Row {
                    icon: Icon::Share,
                    label: name.clone(),
                    detail: String::new(),
                    right: String::new(),
                },
                Item::Dir(name) => Row {
                    icon: Icon::Folder,
                    label: name.clone(),
                    detail: String::new(),
                    right: String::new(),
                },
                Item::Video {
                    name,
                    size,
                    assessment,
                    broken,
                } => Row {
                    icon: match (assessment, broken) {
                        (_, Some(_)) => Icon::Broken,
                        (Some(a), None) => Icon::Video(Some(a.verdict)),
                        (None, None) => Icon::Video(None),
                    },
                    label: name.clone(),
                    detail: match (assessment, broken) {
                        (_, Some(_)) => "Can't read this file".into(),
                        (Some(a), None) => a.title.clone(),
                        (None, None) => "Checking…".into(),
                    },
                    right: format_size(*size),
                },
            })
            .collect();
        self.view.clamp_scroll();
        self.dirty = true;
    }

    fn dialog(&mut self, title: impl Into<String>, body: Vec<String>) {
        self.view.dialog = Some(Dialog {
            title: title.into(),
            body,
            button: "OK".into(),
        });
        self.dirty = true;
    }

    /// Handles worker results; returns a video ready to play.
    pub fn poll(&mut self) -> Option<Box<Opened>> {
        while let Some(response) = self.library.try_recv() {
            match response {
                Response::Shares { id, result } if Some(id) == self.pending => {
                    self.pending = None;
                    match result {
                        Ok(shares) if shares.is_empty() => {
                            self.set_status("This server has no shared folders you can open.")
                        }
                        Ok(shares) => {
                            self.view.status = None;
                            self.items = shares.into_iter().map(Item::Share).collect();
                            self.rebuild_rows();
                        }
                        Err(e) => self.set_status(format!("{e}\n\nPress B to go back.")),
                    }
                }
                Response::List { id, result } if Some(id) == self.pending => {
                    self.pending = None;
                    let Location::Folder {
                        server,
                        share,
                        path,
                    } = self.location.clone()
                    else {
                        continue;
                    };
                    match result {
                        Ok(entries) => {
                            self.items = entries
                                .into_iter()
                                .filter(|e| e.is_dir || is_video(&e.name))
                                .filter(|e| !e.name.starts_with('.'))
                                .map(|e| {
                                    if e.is_dir {
                                        Item::Dir(e.name)
                                    } else {
                                        Item::Video {
                                            name: e.name,
                                            size: e.size,
                                            assessment: None,
                                            broken: None,
                                        }
                                    }
                                })
                                .collect();
                            for item in &self.items {
                                if let Item::Video { name, .. } = item {
                                    let mut file = path.clone();
                                    file.push(name.clone());
                                    self.library.send(Request::Probe {
                                        generation: self.generation,
                                        server,
                                        share: share.clone(),
                                        path: file,
                                    });
                                }
                            }
                            self.view.status = if self.items.is_empty() {
                                Some("No videos or folders here.".into())
                            } else {
                                None
                            };
                            self.rebuild_rows();
                        }
                        Err(e) => self.set_status(format!(
                            "Can't open this folder: {e}\n\nPress B to go back."
                        )),
                    }
                }
                Response::Probe {
                    generation,
                    name,
                    result,
                } if generation == self.generation => {
                    for item in &mut self.items {
                        if let Item::Video {
                            name: n,
                            assessment,
                            broken,
                            ..
                        } = item
                            && *n == name
                        {
                            match &result {
                                Ok(a) => *assessment = Some(a.clone()),
                                Err(e) => *broken = Some(e.clone()),
                            }
                        }
                    }
                    self.rebuild_rows();
                }
                Response::Opened { id, result } if Some(id) == self.pending => {
                    self.pending = None;
                    self.view.footer = FOOTER.into();
                    self.dirty = true;
                    match result {
                        Ok(opened) if opened.assessment.verdict == Verdict::Unplayable => {
                            let a = opened.assessment.clone();
                            self.dialog(
                                a.title,
                                [a.detail, a.hint].into_iter().flatten().collect(),
                            );
                        }
                        Ok(opened) => return Some(opened),
                        Err(e) => self.dialog("Can't open this video", vec![e]),
                    }
                }
                _ => {} // stale response for a place we already left
            }
        }
        None
    }

    /// Activates row `index`.
    pub fn select(&mut self, index: usize) {
        if self.pending.is_some() && !matches!(self.location, Location::Folder { .. }) {
            return;
        }
        let Some(item) = self.items.get(index) else {
            return;
        };
        match (item, self.location.clone()) {
            (Item::Server(i), _) => self.navigate(Location::Shares { server: *i }),
            (Item::Share(name), Location::Shares { server }) => self.navigate(Location::Folder {
                server,
                share: name.clone(),
                path: Vec::new(),
            }),
            (
                Item::Dir(name),
                Location::Folder {
                    server,
                    share,
                    mut path,
                },
            ) => {
                path.push(name.clone());
                self.navigate(Location::Folder {
                    server,
                    share,
                    path,
                });
            }
            (
                Item::Video {
                    name,
                    assessment,
                    broken,
                    ..
                },
                Location::Folder {
                    server,
                    share,
                    mut path,
                },
            ) => {
                if let Some(e) = broken {
                    let body = vec!["It may be damaged or not a video.".into(), e.clone()];
                    self.dialog("Can't read this file", body);
                } else if let Some(a) = assessment
                    .as_ref()
                    .filter(|a| a.verdict == Verdict::Unplayable)
                {
                    let body = [a.detail.clone(), a.hint.clone()]
                        .into_iter()
                        .flatten()
                        .collect();
                    self.dialog(a.title.clone(), body);
                } else {
                    path.push(name.clone());
                    self.view.footer = format!("Opening {name}…");
                    self.dirty = true;
                    let id = self.request_id();
                    self.library.send(Request::Open {
                        id,
                        server,
                        share,
                        path,
                    });
                }
            }
            _ => {}
        }
    }

    pub fn dialog_open(&self) -> bool {
        self.view.dialog.is_some()
    }

    pub fn close_dialog(&mut self) {
        self.view.dialog = None;
        self.dirty = true;
    }

    /// Goes up one level (or closes a dialog). False at the top level.
    pub fn back(&mut self) -> bool {
        if self.dialog_open() {
            self.close_dialog();
            return true;
        }
        match self.location.clone() {
            Location::Servers => return false,
            Location::Shares { .. } => self.show_servers(),
            Location::Folder {
                server,
                share,
                mut path,
            } => {
                if path.pop().is_some() {
                    self.navigate(Location::Folder {
                        server,
                        share,
                        path,
                    });
                } else {
                    self.navigate(Location::Shares { server });
                }
            }
        }
        true
    }

    /// Call after playback returns to the browser.
    pub fn redraw(&mut self) {
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_extensions() {
        assert!(is_video("clip_180_LR.MP4"));
        assert!(is_video("movie.mkv"));
        assert!(!is_video("notes.txt"));
        assert!(!is_video("mkv"));
    }

    #[test]
    fn empty_server_list_explains_setup() {
        let nav = Navigator::new(Library::start(Vec::new(), None));
        assert!(nav.view().status.as_deref().unwrap().contains("add-server"));
    }
}
