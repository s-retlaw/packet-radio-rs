//! File picker modal widget for selecting WAV files in the TUI.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use std::path::{Path, PathBuf};

use super::dialog::centered_rect;

/// A single entry in the file picker list.
#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Self-contained state for the file picker modal.
#[derive(Debug)]
pub struct FilePickerState {
    pub current_dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub selected: usize,
    pub list_state: ListState,
    /// false = show only `.wav` files, true = show all files
    pub show_all: bool,
    pub error: Option<String>,
}

impl FilePickerState {
    /// Create a new file picker starting at the given directory (or cwd).
    pub fn new(start_dir: Option<&Path>) -> Self {
        let dir = start_dir
            .and_then(|p| if p.is_dir() { Some(p.to_path_buf()) } else { p.parent().map(|pp| pp.to_path_buf()) })
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")));

        let mut state = Self {
            current_dir: dir,
            entries: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            show_all: false,
            error: None,
        };
        state.scan_directory();
        state
    }

    /// Scan the current directory and populate entries.
    pub fn scan_directory(&mut self) {
        self.entries.clear();
        self.error = None;

        // Parent directory entry (unless at root)
        if self.current_dir.parent().is_some() {
            self.entries.push(FileEntry {
                name: "..".to_string(),
                is_dir: true,
                size: 0,
            });
        }

        match std::fs::read_dir(&self.current_dir) {
            Ok(read_dir) => {
                let mut dirs = Vec::new();
                let mut files = Vec::new();

                for entry in read_dir.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    // Skip dotfiles
                    if name.starts_with('.') {
                        continue;
                    }

                    let metadata = entry.metadata();
                    let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);

                    if is_dir {
                        dirs.push(FileEntry { name, is_dir: true, size: 0 });
                    } else {
                        // Filter by extension unless show_all
                        if !self.show_all {
                            let lower = name.to_lowercase();
                            if !lower.ends_with(".wav") {
                                continue;
                            }
                        }
                        files.push(FileEntry { name, is_dir: false, size });
                    }
                }

                // Sort alphabetically (case-insensitive)
                dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
                files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

                self.entries.extend(dirs);
                self.entries.extend(files);
            }
            Err(e) => {
                self.error = Some(format!("Cannot read directory: {e}"));
            }
        }

        // Reset selection
        self.selected = 0;
        self.list_state.select(Some(0));
    }

    /// Enter the selected item. Returns Some(path) if a file was selected.
    pub fn enter(&mut self) -> Option<PathBuf> {
        let entry = self.entries.get(self.selected)?;

        if entry.name == ".." {
            self.go_up();
            return None;
        }

        let full_path = self.current_dir.join(&entry.name);

        if entry.is_dir {
            self.current_dir = full_path;
            self.scan_directory();
            None
        } else {
            Some(full_path)
        }
    }

    /// Navigate to parent directory.
    pub fn go_up(&mut self) {
        if let Some(parent) = self.current_dir.parent() {
            self.current_dir = parent.to_path_buf();
            self.scan_directory();
        }
    }

    /// Toggle between WAV-only and all-files filter.
    pub fn toggle_filter(&mut self) {
        self.show_all = !self.show_all;
        self.scan_directory();
    }

    pub fn select_next(&mut self) {
        if !self.entries.is_empty() {
            self.selected = (self.selected + 1) % self.entries.len();
            self.list_state.select(Some(self.selected));
        }
    }

    pub fn select_prev(&mut self) {
        if !self.entries.is_empty() {
            if self.selected == 0 {
                self.selected = self.entries.len() - 1;
            } else {
                self.selected -= 1;
            }
            self.list_state.select(Some(self.selected));
        }
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.list_state.select(Some(0));
    }

    pub fn select_last(&mut self) {
        if !self.entries.is_empty() {
            self.selected = self.entries.len() - 1;
            self.list_state.select(Some(self.selected));
        }
    }
}

/// Format a file size in human-readable form.
fn format_size(size: u64) -> String {
    if size < 1024 {
        format!("{size} B")
    } else if size < 1024 * 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    }
}

/// Draw the file picker as a centered modal overlay.
pub fn draw_file_picker(frame: &mut Frame, area: Rect, state: &mut FilePickerState) {
    let title = if state.show_all { " Open File " } else { " Open WAV File " };
    let width = 70.min(area.width.saturating_sub(4));
    let height = 24.min(area.height.saturating_sub(4));
    let dialog_area = centered_rect(width, height, area);

    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(dialog_area);
    frame.render_widget(block, dialog_area);

    // Layout: path bar (1 line) + file list (remaining) + footer hints (1 line)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // path bar
            Constraint::Min(1),   // file list
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // Path bar
    let path_str = state.current_dir.display().to_string();
    let path_line = Paragraph::new(path_str)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(path_line, chunks[0]);

    // Error or file list
    if let Some(ref err) = state.error {
        let err_para = Paragraph::new(err.as_str())
            .style(Style::default().fg(Color::Red));
        frame.render_widget(err_para, chunks[1]);
    } else {
        let items: Vec<ListItem> = state
            .entries
            .iter()
            .map(|entry| {
                if entry.is_dir {
                    ListItem::new(format!("[DIR] {}", entry.name))
                        .style(Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD))
                } else {
                    ListItem::new(format!("      {}  {}", entry.name, format_size(entry.size)))
                }
            })
            .collect();

        let list = List::new(items)
            .highlight_style(Style::default().bg(Color::DarkGray).fg(Color::White))
            .highlight_symbol("> ");

        frame.render_stateful_widget(list, chunks[1], &mut state.list_state);
    }

    // Footer hints
    let filter_hint = if state.show_all { "a:WAV only" } else { "a:Show all" };
    let hints = format!(" Enter:Open  Esc:Cancel  Bksp:Up  {filter_hint}");
    let footer = Paragraph::new(hints)
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Create a temporary directory with test files (unique per call).
    fn setup_test_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("packet-radio-file-picker-test-{id}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // Create subdirectories
        fs::create_dir_all(dir.join("subdir")).unwrap();
        fs::create_dir_all(dir.join("another_dir")).unwrap();

        // Create files
        fs::write(dir.join("track1.wav"), b"RIFF").unwrap();
        fs::write(dir.join("track2.wav"), b"RIFF1234").unwrap();
        fs::write(dir.join("readme.txt"), b"hello").unwrap();
        fs::write(dir.join("data.bin"), b"binary").unwrap();

        // Create dotfile (should be hidden)
        fs::write(dir.join(".hidden"), b"secret").unwrap();

        dir
    }

    fn cleanup_test_dir(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn test_scan_directory_wav_filter() {
        let dir = setup_test_dir();
        let state = FilePickerState::new(Some(&dir));

        // Should have: "..", "another_dir", "subdir", "track1.wav", "track2.wav"
        // (dirs sorted alpha, then WAV files only)
        assert!(!state.entries.is_empty());

        let dir_names: Vec<&str> = state.entries.iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name.as_str())
            .collect();
        assert!(dir_names.contains(&".."));
        assert!(dir_names.contains(&"subdir"));
        assert!(dir_names.contains(&"another_dir"));

        let file_names: Vec<&str> = state.entries.iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.name.as_str())
            .collect();
        assert!(file_names.contains(&"track1.wav"));
        assert!(file_names.contains(&"track2.wav"));
        // Non-WAV files should be excluded
        assert!(!file_names.contains(&"readme.txt"));
        assert!(!file_names.contains(&"data.bin"));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_navigate_enter_dir() {
        let dir = setup_test_dir();
        let mut state = FilePickerState::new(Some(&dir));

        // Find "subdir" and navigate into it
        let subdir_idx = state.entries.iter()
            .position(|e| e.name == "subdir")
            .unwrap();
        state.selected = subdir_idx;
        state.list_state.select(Some(subdir_idx));

        let result = state.enter();
        assert!(result.is_none()); // navigated into dir, not a file
        assert!(state.current_dir.ends_with("subdir"));

        // Go back up
        state.go_up();
        assert_eq!(state.current_dir, dir);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_toggle_filter() {
        let dir = setup_test_dir();
        let mut state = FilePickerState::new(Some(&dir));

        let wav_count = state.entries.iter().filter(|e| !e.is_dir).count();

        state.toggle_filter();
        assert!(state.show_all);
        let all_count = state.entries.iter().filter(|e| !e.is_dir).count();
        assert!(all_count >= wav_count);
        // Should now include readme.txt, data.bin
        let file_names: Vec<&str> = state.entries.iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.name.as_str())
            .collect();
        assert!(file_names.contains(&"readme.txt"));
        assert!(file_names.contains(&"data.bin"));

        // Toggle back
        state.toggle_filter();
        assert!(!state.show_all);
        let wav_count2 = state.entries.iter().filter(|e| !e.is_dir).count();
        assert_eq!(wav_count, wav_count2);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_select_file() {
        let dir = setup_test_dir();
        let mut state = FilePickerState::new(Some(&dir));

        // Find "track1.wav" and select it
        let file_idx = state.entries.iter()
            .position(|e| e.name == "track1.wav")
            .unwrap();
        state.selected = file_idx;
        state.list_state.select(Some(file_idx));

        let result = state.enter();
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(path.ends_with("track1.wav"));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_dotfiles_hidden() {
        let dir = setup_test_dir();
        let state = FilePickerState::new(Some(&dir));

        let names: Vec<&str> = state.entries.iter().map(|e| e.name.as_str()).collect();
        assert!(!names.contains(&".hidden"));

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_empty_directory() {
        let dir = std::env::temp_dir().join("packet-radio-file-picker-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let state = FilePickerState::new(Some(&dir));
        // Should have just ".." entry (parent nav)
        assert!(!state.entries.is_empty());
        assert_eq!(state.entries[0].name, "..");

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_cursor_navigation() {
        let dir = setup_test_dir();
        let mut state = FilePickerState::new(Some(&dir));
        let len = state.entries.len();
        assert!(len > 2);

        assert_eq!(state.selected, 0);

        state.select_next();
        assert_eq!(state.selected, 1);

        state.select_prev();
        assert_eq!(state.selected, 0);

        // Wrap around backward
        state.select_prev();
        assert_eq!(state.selected, len - 1);

        // Wrap around forward
        state.select_next();
        assert_eq!(state.selected, 0);

        state.select_last();
        assert_eq!(state.selected, len - 1);

        state.select_first();
        assert_eq!(state.selected, 0);

        cleanup_test_dir(&dir);
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
    }

    #[test]
    fn test_dirs_before_files() {
        let dir = setup_test_dir();
        let state = FilePickerState::new(Some(&dir));

        // All directories should come before all files
        let mut seen_file = false;
        for entry in &state.entries {
            if !entry.is_dir {
                seen_file = true;
            } else if seen_file {
                panic!("Directory '{}' appeared after a file", entry.name);
            }
        }

        cleanup_test_dir(&dir);
    }
}
