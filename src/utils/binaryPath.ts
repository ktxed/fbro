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

/** Extensions (and dotless names) a developer expects to edit in the built-in editor. */
const DEV_TEXT_EXTENSIONS = new Set([
  "txt", "md", "mdx", "markdown", "rst", "adoc", "log", "csv", "tsv",
  "yml", "yaml", "json", "jsonc", "json5", "toml", "ini", "cfg", "conf", "env", "properties",
  "xml", "html", "htm", "xhtml", "svg", "css", "scss", "sass", "less",
  "ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "vue", "svelte", "astro",
  "rs", "go", "py", "rb", "php", "java", "kt", "kts", "scala", "swift", "dart", "lua", "pl", "r",
  "c", "h", "cc", "cpp", "cxx", "hpp", "hh", "hxx", "cs", "m", "mm",
  "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd",
  "sql", "graphql", "gql", "proto", "tf", "hcl", "gradle", "cmake", "mk", "lock", "diff", "patch",
]);

const DEV_TEXT_NAMES = new Set([
  "makefile", "dockerfile", "readme", "license", "changelog", "gemfile", "rakefile", "justfile", "cargo.lock",
]);

/** True for source/config/text files worth offering "Edit" for (Enter would otherwise open an OS handler). */
export function isDeveloperTextPath(path: string): boolean {
  const base = (path.split(/[/\\]/).pop() ?? path).toLowerCase();
  if (DEV_TEXT_NAMES.has(base)) return true;
  if (base.startsWith(".") && base.indexOf(".", 1) < 0) return true; // dotfiles: .gitignore, .bashrc
  return DEV_TEXT_EXTENSIONS.has(extensionOf(base));
}
