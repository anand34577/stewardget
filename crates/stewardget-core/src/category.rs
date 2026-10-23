use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// File category, derived from the file name: one of the built-in kinds, or a
/// category the user defined (see [`crate::Config::custom_categories`]).
///
/// Stored as a string (`"video"`, `"custom-3"`), so it also works as a map key
/// in `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Category {
    Video,
    Audio,
    Image,
    Document,
    Archive,
    Program,
    #[default]
    Other,
    /// A user-defined category, by its id.
    Custom(u32),
}

impl Category {
    /// The built-in categories, `Other` last.
    pub const ALL: [Category; 7] = [
        Category::Video,
        Category::Audio,
        Category::Image,
        Category::Document,
        Category::Archive,
        Category::Program,
        Category::Other,
    ];

    /// The built-in category of a file name (custom categories are applied by
    /// [`crate::Config::categorize`]).
    pub fn from_file_name(name: &str) -> Self {
        let lower = name.to_ascii_lowercase();
        // multi-part extensions first
        if [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"].iter().any(|e| lower.ends_with(e)) {
            return Category::Archive;
        }
        let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "mp4" | "mkv" | "webm" | "avi" | "mov" | "wmv" | "flv" | "m4v" | "mpg" | "mpeg" | "3gp" | "ts" => {
                Category::Video
            }
            "mp3" | "flac" | "wav" | "aac" | "ogg" | "opus" | "m4a" | "wma" | "alac" | "aiff" => Category::Audio,
            "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" | "tiff" | "tif" | "heic" | "avif" | "ico"
            | "raw" | "psd" => Category::Image,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "txt" | "md" | "rtf"
            | "epub" | "mobi" | "csv" | "json" | "xml" => Category::Document,
            "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "lz4" | "lzma" | "cab" | "iso"
            | "img" => Category::Archive,
            "exe" | "msi" | "msix" | "dmg" | "pkg" | "deb" | "rpm" | "appimage" | "apk" | "flatpak" | "snap"
            | "run" | "bin" | "jar" => Category::Program,
            _ => Category::Other,
        }
    }

    /// Human readable, plural label of a built-in category (also its sub-folder
    /// name). Custom categories are named in the configuration; see
    /// [`crate::Config::category_label`].
    pub fn label(self) -> &'static str {
        match self {
            Category::Video => "Video",
            Category::Audio => "Music",
            Category::Image => "Images",
            Category::Document => "Documents",
            Category::Archive => "Archives",
            Category::Program => "Programs",
            Category::Other => "Other",
            Category::Custom(_) => "Custom",
        }
    }

    /// Sub-folder of a built-in category when auto-categorisation is on. `Other`
    /// stays in the root.
    pub fn folder(self) -> Option<&'static str> {
        (!matches!(self, Category::Other | Category::Custom(_))).then(|| self.label())
    }

    fn key(self) -> std::borrow::Cow<'static, str> {
        match self {
            Category::Video => "video".into(),
            Category::Audio => "audio".into(),
            Category::Image => "image".into(),
            Category::Document => "document".into(),
            Category::Archive => "archive".into(),
            Category::Program => "program".into(),
            Category::Other => "other".into(),
            Category::Custom(id) => format!("custom-{id}").into(),
        }
    }

    fn from_key(key: &str) -> Self {
        match key {
            "video" => Category::Video,
            "audio" => Category::Audio,
            "image" => Category::Image,
            "document" => Category::Document,
            "archive" => Category::Archive,
            "program" => Category::Program,
            k => k.strip_prefix("custom-").and_then(|id| id.parse().ok()).map_or(Category::Other, Category::Custom),
        }
    }
}

impl Serialize for Category {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.key())
    }
}

impl<'de> Deserialize<'de> for Category {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Unknown values (a newer version's category) fall back to `Other`.
        Ok(Category::from_key(&String::deserialize(d)?))
    }
}

/// A category the user defined: files with one of its extensions get its name,
/// sidebar entry and folder. Custom categories are checked before the built-in ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomCategory {
    pub id: u32,
    pub name: String,
    /// Lowercase extensions without the dot; may have several parts (`tar.gz`).
    pub extensions: Vec<String>,
}

impl Default for CustomCategory {
    fn default() -> Self {
        Self { id: 1, name: String::new(), extensions: Vec::new() }
    }
}

impl CustomCategory {
    /// Whether a (lowercase) file name ends in one of the extensions.
    pub fn matches(&self, lower_name: &str) -> bool {
        self.extensions.iter().any(|ext| {
            lower_name.len() > ext.len() + 1
                && lower_name.ends_with(ext.as_str())
                && lower_name.as_bytes()[lower_name.len() - ext.len() - 1] == b'.'
        })
    }

    /// Parses a typed list such as `"iso, .img *.vhdx"` into clean extensions.
    pub fn parse_extensions(text: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for raw in text.split([',', ';', ' ', '\n', '\t']) {
            let ext = raw.trim().trim_start_matches('*').trim_start_matches('.').trim().to_lowercase();
            if !ext.is_empty() && !ext.contains(['/', '\\']) && !out.contains(&ext) {
                out.push(ext);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_as_strings() {
        for c in Category::ALL.into_iter().chain([Category::Custom(7)]) {
            let json = serde_json::to_string(&c).unwrap();
            assert_eq!(serde_json::from_str::<Category>(&json).unwrap(), c);
        }
        assert_eq!(serde_json::to_string(&Category::Custom(3)).unwrap(), "\"custom-3\"");
        assert_eq!(serde_json::from_str::<Category>("\"video\"").unwrap(), Category::Video);
        assert_eq!(serde_json::from_str::<Category>("\"hologram\"").unwrap(), Category::Other);
        // Works as a map key, which `category_dirs` needs.
        let map = std::collections::BTreeMap::from([(Category::Custom(2), 1), (Category::Audio, 2)]);
        let json = serde_json::to_string(&map).unwrap();
        assert_eq!(serde_json::from_str::<std::collections::BTreeMap<Category, i32>>(&json).unwrap(), map);
    }

    #[test]
    fn custom_extensions() {
        let c = CustomCategory {
            id: 1,
            name: "Disk images".into(),
            extensions: CustomCategory::parse_extensions("ISO, .img *.vhdx tar.gz iso"),
        };
        assert_eq!(c.extensions, ["iso", "img", "vhdx", "tar.gz"]);
        assert!(c.matches("ubuntu.iso"));
        assert!(c.matches("src.tar.gz"));
        assert!(!c.matches("iso"));
        assert!(!c.matches("fakeiso"));
    }
}
