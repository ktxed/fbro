/** Filename extension of the last path segment, or "" when missing / dotfile. */
function extensionOf(path: string): string {
  const base = path.split(/[/\\]/).pop() ?? path;
  const dot = base.lastIndexOf(".");
  if (dot <= 0) return "";
  return base.slice(dot + 1).toLowerCase();
}

/** Extensions whose contents are not useful as a text/Git diff. SVG is omitted — it is XML. */
const BINARY_EXTENSIONS = new Set([
  // images
  "png", "jpg", "jpeg", "jpe", "jfif", "bmp", "gif", "webp", "avif",
  "ico", "icns", "tif", "tiff", "heic", "heif", "psd", "raw", "cr2", "nef", "dng", "arw",
  // video
  "mp4", "m4v", "mov", "qt", "avi", "mkv", "webm", "wmv", "flv", "mpeg", "mpg", "mpe",
  "3gp", "3g2", "ogv", "ogm", "mts", "m2ts", "vob", "asf",
  // audio
  "mp3", "flac", "wav", "aac", "m4a", "wma", "opus", "ogg", "oga", "aif", "aiff",
  "ac3", "mka", "ape", "amr",
  // documents
  "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "epub",
  // archives / disk images
  "zip", "7z", "rar", "tar", "gz", "bz2", "xz", "zst", "tgz", "tbz", "tbz2", "txz",
  "lz", "lzma", "cab", "iso", "dmg", "apk", "jar", "war", "ear", "xpi", "crx", "deb", "rpm",
  // fonts
  "ttf", "otf", "woff", "woff2", "eot",
  // compiled / object
  "exe", "dll", "so", "dylib", "o", "lib", "wasm", "bin", "appimage", "class", "pyc", "pyo",
  "sqlite", "sqlite3",
]);

/** True when `path` looks like a binary file from its extension alone. */
export function isLikelyBinaryPath(path: string): boolean {
  const ext = extensionOf(path);
  return ext !== "" && BINARY_EXTENSIONS.has(ext);
}
