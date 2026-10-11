//! Solution-Explorer style file tree. Directory listings honor .gitignore
//! (via the `ignore` crate from ripgrep) and are loaded lazily on expand.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub depth: usize,
}

pub struct Explorer {
    pub root: PathBuf,
    pub entries: Vec<Entry>,
    pub expanded: HashSet<PathBuf>,
    pub selected: usize,
    pub scroll: usize,
    listings: HashMap<PathBuf, Vec<(String, bool)>>,
}

fn walker(dir: &Path, depth: usize) -> WalkBuilder {
    let mut b = WalkBuilder::new(dir);
    b.max_depth(Some(depth))
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false);
    b.filter_entry(|e| e.file_name() != ".git");
    b
}

impl Explorer {
    pub fn new(root: PathBuf) -> Self {
        let mut e = Self {
            root,
            entries: Vec::new(),
            expanded: HashSet::new(),
            selected: 0,
            scroll: 0,
            listings: HashMap::new(),
        };
        e.rebuild();
        e
    }

    pub fn root_name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    fn list(&mut self, dir: &Path) -> Vec<(String, bool)> {
        if let Some(v) = self.listings.get(dir) {
            return v.clone();
        }
        let mut v: Vec<(String, bool)> = walker(dir, 1)
            .build()
            .flatten()
            .filter(|e| e.depth() == 1)
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.file_type().is_some_and(|t| t.is_dir()),
                )
            })
            .collect();
        v.sort_by(|a, b| {
            b.1.cmp(&a.1)
                .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
        });
        self.listings.insert(dir.to_path_buf(), v.clone());
        v
    }

    pub fn refresh(&mut self) {
        self.listings.clear();
        self.rebuild();
    }

    pub fn rebuild(&mut self) {
        let mut out = Vec::new();
        let root = self.root.clone();
        self.push_dir(&root, 0, &mut out);
        self.entries = out;
        self.selected = self.selected.min(self.entries.len().saturating_sub(1));
    }

    fn push_dir(&mut self, dir: &Path, depth: usize, out: &mut Vec<Entry>) {
        let items = self.list(dir);
        for (name, is_dir) in items {
            let path = dir.join(&name);
            let open = is_dir && self.expanded.contains(&path);
            out.push(Entry {
                path: path.clone(),
                name,
                is_dir,
                depth,
            });
            if open {
                self.push_dir(&path, depth + 1, out);
            }
        }
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    pub fn toggle(&mut self, idx: usize) {
        if let Some(e) = self.entries.get(idx).cloned()
            && e.is_dir
        {
            if !self.expanded.remove(&e.path) {
                self.expanded.insert(e.path);
            }
            self.rebuild();
        }
    }

    /// Right arrow: expand a folder, or step into it if already open.
    pub fn expand(&mut self) {
        let Some(e) = self.selected_entry().cloned() else {
            return;
        };
        if e.is_dir {
            if self.expanded.contains(&e.path) {
                self.move_selection(1);
            } else {
                self.toggle(self.selected);
            }
        }
    }

    /// Left arrow: collapse an open folder, or jump to the parent folder.
    pub fn collapse(&mut self) {
        let Some(e) = self.selected_entry().cloned() else {
            return;
        };
        if e.is_dir && self.expanded.contains(&e.path) {
            self.toggle(self.selected);
        } else if let Some(parent) = e.path.parent()
            && let Some(p) = self.entries.iter().position(|x| x.path == parent)
        {
            self.selected = p;
        }
    }

    /// Expand parents of `path` and select it.
    pub fn reveal(&mut self, path: &Path) {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return;
        };
        let mut cur = self.root.clone();
        let comps: Vec<_> = rel.components().collect();
        for c in comps.iter().take(comps.len().saturating_sub(1)) {
            cur = cur.join(c);
            self.expanded.insert(cur.clone());
        }
        self.rebuild();
        if let Some(i) = self.entries.iter().position(|e| e.path == path) {
            self.selected = i;
        }
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let max = self.entries.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    pub fn ensure_visible(&mut self, height: usize) {
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if height > 0 && self.selected >= self.scroll + height {
            self.scroll = self.selected + 1 - height;
        }
    }
}

/// All project files (relative paths), for Quick Open.
pub fn project_files(root: &Path, limit: usize) -> Vec<String> {
    let mut files: Vec<String> = walker(root, usize::MAX)
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .take(limit)
        .filter_map(|e| {
            e.path()
                .strip_prefix(root)
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .collect();
    files.sort();
    files
}
