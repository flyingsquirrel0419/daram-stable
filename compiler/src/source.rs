//! Source file management and span tracking.

/// A unique identifier for a source file in the compilation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileId(pub u32);

/// A byte offset within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ByteOffset(pub u32);

/// A half-open byte range `[start, end)` within a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub file: FileId,
    pub start: ByteOffset,
    pub end: ByteOffset,
}

impl Span {
    pub fn new(file: FileId, start: u32, end: u32) -> Self {
        Self {
            file,
            start: ByteOffset(start),
            end: ByteOffset(end),
        }
    }

    /// Merge two spans — result covers from the earliest start to the latest end.
    /// Panics if spans belong to different files (programmer error).
    pub fn merge(self, other: Span) -> Span {
        assert_eq!(
            self.file, other.file,
            "cannot merge spans from different files"
        );
        Span {
            file: self.file,
            start: self.start.min(other.start),
            end: self.end.max(other.end),
        }
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// A single source file loaded into memory.
pub struct SourceFile {
    pub id: FileId,
    pub name: String,
    pub src: String,
    /// Byte offsets of the start of each line, for efficient line/column lookup.
    line_starts: Vec<u32>,
}

impl SourceFile {
    pub fn new(id: FileId, name: String, src: String) -> Self {
        let line_starts = std::iter::once(0u32)
            .chain(src.match_indices('\n').map(|(i, _)| (i + 1) as u32))
            .collect();
        Self {
            id,
            name,
            src,
            line_starts,
        }
    }

    /// Return the 1-based line and 1-based column for a byte offset.
    pub fn line_col(&self, offset: ByteOffset) -> (u32, u32) {
        let off = offset.0;
        let line = match self.line_starts.binary_search(&off) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        };
        let col = off - self.line_starts[line];
        (line as u32 + 1, col + 1)
    }

    /// Return the source text of a span.
    pub fn slice(&self, span: Span) -> &str {
        let s = span.start.0 as usize;
        let e = (span.end.0 as usize).min(self.src.len());
        &self.src[s..e]
    }

    /// Return the full text of a line (1-based), without the trailing newline.
    pub fn line_text(&self, line: u32) -> &str {
        let idx = (line as usize).saturating_sub(1);
        let start = self.line_starts.get(idx).copied().unwrap_or(0) as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .copied()
            .map(|v| v as usize)
            .unwrap_or(self.src.len());
        self.src[start..end].trim_end_matches('\n')
    }
}

/// Registry of all source files in a compilation session.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, name: String, src: String) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.files.push(SourceFile::new(id, name, src));
        id
    }

    pub fn get(&self, id: FileId) -> &SourceFile {
        &self.files[id.0 as usize]
    }

    pub fn try_get(&self, id: FileId) -> Option<&SourceFile> {
        self.files.get(id.0 as usize)
    }
}
