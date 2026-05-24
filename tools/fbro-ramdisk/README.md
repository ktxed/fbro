# fbro RAM Disk Transfer Helper

This helper mounts a Linux `tmpfs` under your home folder, copies a realistic
test file into it, and leaves the file visible to fbro through the normal local
filesystem source.

## Setup

```bash
tools/fbro-ramdisk/fbro_ramdisk.py setup --file /path/to/movie-from-phone.mov
```

Defaults:

- mount point: `$HOME/fbro-ramdisk`
- RAM disk size: `8G`
- file behavior: copy, never move

Useful options:

```bash
tools/fbro-ramdisk/fbro_ramdisk.py setup --file /path/to/movie.mov --size 8G
tools/fbro-ramdisk/fbro_ramdisk.py setup --file /path/to/movie.mov --name test.mov
tools/fbro-ramdisk/fbro_ramdisk.py setup --file /path/to/movie.mov --force
```

Mounting and unmounting call `sudo mount` and `sudo umount`. You do not need
to run the whole script with `sudo`, but if you do, it still targets the
original invoking user's home via `SUDO_USER` and mounts tmpfs with that user's
UID/GID.

If an older version mounted `/root/fbro-ramdisk`, clean it up with:

```bash
sudo umount /root/fbro-ramdisk
sudo rmdir /root/fbro-ramdisk
```

Do not symlink your home folder to `/root/fbro-ramdisk`; root's home is not a
good source path for fbro, and the permissions are wrong for the normal user.

## fbro Test

1. On the Linux fbro instance, enable remote access and share the filesystem
   source that contains `$HOME`.
2. On the Mac fbro client, connect to the Linux peer.
3. Navigate to `~/fbro-ramdisk`.
4. Copy the preloaded file to a local Mac destination.
5. Record the fbro job throughput and elapsed time.

## Status

```bash
tools/fbro-ramdisk/fbro_ramdisk.py status
```

This prints whether the mount point exists, whether it is mounted, the
filesystem type, capacity, and files currently in the RAM disk.

## Cleanup

```bash
tools/fbro-ramdisk/fbro_ramdisk.py cleanup --remove-files
```

Without `--remove-files`, cleanup refuses to unmount a non-empty RAM disk so the
discard is explicit. If unmount reports the target is busy, close fbro panels or
shells inside `~/fbro-ramdisk` and retry.
