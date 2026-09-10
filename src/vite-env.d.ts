declare module "*.css?inline" {
  const css: string;
  export default css;
}

declare module "*.gif" {
  const url: string;
  export default url;
}

interface FbroBuildInfo {
  version: string;
  commit: string;
  shortCommit: string;
  branch: string | null;
  tag: string | null;
  dirty: boolean | null;
  builtAt: string;
  profile: "development" | "debug" | "release";
}

declare const __FBRO_BUILD_INFO__: Readonly<FbroBuildInfo>;
