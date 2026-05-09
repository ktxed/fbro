export function SettingsDisplayTab() {
  return (
    <div className="settings-tab">
      <h3>Thumbnails</h3>
      <p className="settings-tab__hint">
        Grid view (Shift+G) shows file thumbnails using the OS-native provider
        on Windows and macOS. Linux falls back to file-type icons. An
        fbro-managed thumbnailer with a configurable on-disk cache and video
        frame extraction is planned for a future release.
      </p>
    </div>
  );
}
