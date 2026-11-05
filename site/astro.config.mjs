// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

// Published by .github/workflows/pages.yml to https://ktxed.github.io/fbro/.
export default defineConfig({
  site: "https://ktxed.github.io",
  base: "/fbro",
  integrations: [
    starlight({
      title: "fbro",
      description: "User guide for fbro, a keyboard-first dual-pane file manager.",
      logo: { src: "./src/assets/logo.png", alt: "fbro", replacesTitle: true },
      favicon: "/favicon.png",
      social: [{ icon: "github", label: "GitHub", href: "https://github.com/ktxed/fbro" }],
      editLink: { baseUrl: "https://github.com/ktxed/fbro/edit/development/site/" },
      customCss: ["./src/styles/fbro.css"],
      sidebar: [
        {
          label: "Get started",
          items: [
            "get-started/install",
            "get-started/first-launch",
            "get-started/orthodox-101",
            "get-started/tour",
          ],
        },
        {
          label: "Using fbro",
          items: [
            "using/navigation",
            "using/selection",
            "using/sorting-filtering",
            "using/tabs",
            "using/sources",
            "using/bookmarks-history",
            "using/views",
            "using/command-palette",
          ],
        },
        {
          label: "File operations",
          items: [
            "files/operations",
            "files/jobs",
            "files/inline-choices",
            "files/clipboard-drag",
            "files/open-with",
          ],
        },
        {
          label: "Viewing & editing",
          items: [
            "viewing/viewer",
            "viewing/quick-preview",
            "viewing/compare",
            "viewing/editor",
            "viewing/terminal",
          ],
        },
        {
          label: "Bros",
          items: ["bros/overview", "bros/archives", "bros/hashes", "bros/duplicates", "bros/more"],
        },
        { label: "Git", items: ["git/repositories"] },
        {
          label: "Remote fbro",
          items: ["remote/pairing", "remote/browse-transfer", "remote/sync", "remote/troubleshooting"],
        },
        {
          label: "Customize",
          items: ["customize/settings", "customize/color-schemes", "customize/notifications"],
        },
        { label: "Reference", items: ["reference/keyboard-shortcuts"] },
        { label: "Help", items: ["help/faq"] },
      ],
    }),
  ],
});
