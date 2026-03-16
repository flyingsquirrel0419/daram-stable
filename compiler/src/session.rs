//! Compilation session — holds global state for a single compiler invocation.

use crate::source::SourceMap;

pub struct Session {
    pub source_map: SourceMap,
}

impl Session {
    pub fn new() -> Self {
        Self {
            source_map: SourceMap::new(),
        }
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}
