declare module "*.css?inline" {
  const css: string;
  export default css;
}

declare module "*.gif" {
  const url: string;
  export default url;
}
