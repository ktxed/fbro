# Project context

`fbro` is a n-panel "orthodox" file manager

In the `spex` folder I have defined the project's specification

`spec.md` - overview
`proto.md` - client-server communication (FE and BE)
`vfs.md` - treats virtual file system driver (for abstracting filesystems)

## Technologies

The idea is for fbro to be developed using the Tauri framework.
This means the BE part is Rust.
The FE part is React/TypeScript.
FE and BE communicate using a Websocket