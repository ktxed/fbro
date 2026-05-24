#!/usr/bin/env python3
"""Linux tmpfs helper for fbro remote transfer benchmarks."""

from __future__ import annotations

import argparse
import os
import platform
import pwd
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path


DEFAULT_MOUNT_NAME = "fbro-ramdisk"
DEFAULT_SIZE = "8G"


@dataclass(frozen=True)
class UserContext:
    uid: int
    gid: int
    home: Path
    name: str


def fail(message: str) -> None:
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(1)


def require_linux() -> None:
    if platform.system() != "Linux":
        fail("this helper is Linux-only")


def target_user() -> UserContext:
    sudo_uid = os.environ.get("SUDO_UID")
    sudo_user = os.environ.get("SUDO_USER")
    if sudo_uid and sudo_user and sudo_user != "root":
        try:
            pw = pwd.getpwnam(sudo_user)
            return UserContext(
                uid=int(sudo_uid),
                gid=pw.pw_gid,
                home=Path(pw.pw_dir),
                name=sudo_user,
            )
        except (KeyError, ValueError):
            pass

    uid = os.getuid()
    pw = pwd.getpwuid(uid)
    return UserContext(uid=uid, gid=pw.pw_gid, home=Path(pw.pw_dir), name=pw.pw_name)


def expand_path(raw: str, user: UserContext) -> Path:
    if raw == "~":
        return user.home.resolve()
    if raw.startswith("~/"):
        return (user.home / raw[2:]).resolve()
    return Path(raw).expanduser().resolve()


def default_mount_point(user: UserContext) -> Path:
    home = user.home
    if not str(home):
        fail("could not determine home directory")
    return home / DEFAULT_MOUNT_NAME


def run(cmd: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=check, text=True)


def capture(cmd: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=check, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def is_mountpoint(path: Path) -> bool:
    return run(["mountpoint", "-q", str(path)], check=False).returncode == 0


def mount_fstype(path: Path) -> str | None:
    if not is_mountpoint(path):
        return None
    result = capture(["findmnt", "-n", "-o", "FSTYPE", "--target", str(path)], check=False)
    if result.returncode != 0:
        return None
    value = result.stdout.strip()
    return value or None


def human_bytes(value: int) -> str:
    units = ["B", "KiB", "MiB", "GiB", "TiB"]
    size = float(value)
    for unit in units:
        if size < 1024 or unit == units[-1]:
            if unit == "B":
                return f"{int(size)} {unit}"
            return f"{size:.1f} {unit}"
        size /= 1024
    return f"{value} B"


def available_bytes(path: Path) -> int:
    usage = shutil.disk_usage(path)
    return usage.free


def print_capacity(path: Path) -> None:
    result = capture(["df", "-h", str(path)], check=False)
    if result.returncode == 0 and result.stdout.strip():
        print(result.stdout.rstrip())
        return
    usage = shutil.disk_usage(path)
    print(
        "capacity: "
        f"total={human_bytes(usage.total)} "
        f"used={human_bytes(usage.used)} "
        f"free={human_bytes(usage.free)}"
    )


def validate_input_file(path: Path) -> None:
    if not path.exists():
        fail(f"input file does not exist: {path}")
    if not path.is_file():
        fail(f"input path is not a regular file: {path}")


def chown_for_target_user(path: Path, user: UserContext, *, recursive: bool = False) -> None:
    try:
        st = path.stat()
        if st.st_uid == user.uid and st.st_gid == user.gid and not recursive:
            return
    except OSError:
        return

    if os.geteuid() == 0:
        if recursive:
            shutil.chown(path, user=user.uid, group=user.gid)
            for root, dirs, files in os.walk(path):
                for name in dirs + files:
                    shutil.chown(Path(root) / name, user=user.uid, group=user.gid)
        else:
            os.chown(path, user.uid, user.gid)
        return

    target = f"{user.uid}:{user.gid}"
    if recursive:
        run(["sudo", "chown", "-R", target, str(path)])
    else:
        run(["sudo", "chown", target, str(path)])


def ensure_tmpfs_mount(path: Path, size: str, force: bool, user: UserContext) -> None:
    path.mkdir(parents=True, exist_ok=True)
    chown_for_target_user(path, user)

    if is_mountpoint(path):
        fstype = mount_fstype(path)
        if fstype == "tmpfs":
            chown_for_target_user(path, user, recursive=True)
            print(f"tmpfs already mounted at {path}")
            return
        message = f"{path} is already mounted"
        if fstype:
            message += f" as {fstype}"
        if not force:
            fail(f"{message}; pass --force to unmount and replace it")
        print(f"{message}; unmounting because --force was passed")
        run(["sudo", "umount", str(path)])

    options = f"size={size},mode=0700,uid={user.uid},gid={user.gid}"
    run(["sudo", "mount", "-t", "tmpfs", "-o", options, "tmpfs", str(path)])
    print(f"mounted tmpfs at {path} ({size}, owner {user.name}:{user.gid})")


def copy_to_ramdisk(
    input_path: Path,
    mount_point: Path,
    name: str | None,
    force: bool,
    user: UserContext,
) -> Path:
    output_name = name or input_path.name
    if output_name in {"", ".", ".."} or "/" in output_name:
        fail("--name must be a plain file name")

    output_path = mount_point / output_name
    if output_path.exists() and not force:
        fail(f"destination already exists: {output_path}; pass --force to overwrite")

    size = input_path.stat().st_size
    free = available_bytes(mount_point)
    if output_path.exists():
        free += output_path.stat().st_size
    if size > free:
        fail(
            f"input file is {human_bytes(size)}, but only {human_bytes(free)} "
            f"is available in {mount_point}"
        )

    print(f"copying {input_path} -> {output_path}")
    shutil.copy2(input_path, output_path)
    chown_for_target_user(output_path, user)

    copied_size = output_path.stat().st_size
    if copied_size != size:
        fail(f"copied size mismatch: expected {size} bytes, got {copied_size} bytes")

    return output_path


def cmd_setup(args: argparse.Namespace) -> None:
    require_linux()
    user = target_user()
    input_path = expand_path(args.file, user)
    mount_point = expand_path(args.mount_point, user)
    validate_input_file(input_path)

    ensure_tmpfs_mount(mount_point, args.size, args.force, user)
    if mount_fstype(mount_point) != "tmpfs":
        fail(f"{mount_point} is not mounted as tmpfs")

    output_path = copy_to_ramdisk(input_path, mount_point, args.name, args.force, user)
    print()
    print(f"ready for fbro: {output_path}")
    print_capacity(mount_point)


def cmd_status(args: argparse.Namespace) -> None:
    require_linux()
    user = target_user()
    mount_point = expand_path(args.mount_point, user)
    print(f"user: {user.name} ({user.uid}:{user.gid})")
    print(f"mount point: {mount_point}")
    print(f"exists: {'yes' if mount_point.exists() else 'no'}")
    if not mount_point.exists():
        return

    mounted = is_mountpoint(mount_point)
    print(f"mounted: {'yes' if mounted else 'no'}")
    if mounted:
        print(f"filesystem: {mount_fstype(mount_point) or 'unknown'}")
        print_capacity(mount_point)

    try:
        files = sorted(mount_point.iterdir(), key=lambda p: p.name.lower()) if mount_point.is_dir() else []
    except PermissionError:
        print("files: permission denied")
        print(f"hint: sudo chown -R {user.uid}:{user.gid} {mount_point}")
        return
    if not files:
        print("files: none")
        return

    print("files:")
    for item in files:
        if item.is_file():
            print(f"  {item.name}\t{human_bytes(item.stat().st_size)}")
        elif item.is_dir():
            print(f"  {item.name}/")
        else:
            print(f"  {item.name}")


def remove_mount_contents(path: Path) -> None:
    for item in path.iterdir():
        if item.is_dir() and not item.is_symlink():
            shutil.rmtree(item)
        else:
            item.unlink()


def cmd_cleanup(args: argparse.Namespace) -> None:
    require_linux()
    user = target_user()
    mount_point = expand_path(args.mount_point, user)
    if not mount_point.exists():
        print(f"{mount_point} does not exist")
        return

    mounted = is_mountpoint(mount_point)
    has_contents = mount_point.is_dir() and any(mount_point.iterdir())
    if has_contents and not args.remove_files:
        fail(
            f"{mount_point} contains files; pass --remove-files to delete them "
            "before unmounting"
        )
    if has_contents and args.remove_files:
        remove_mount_contents(mount_point)
        print(f"removed contents from {mount_point}")

    if mounted:
        run(["sudo", "umount", str(mount_point)])
        print(f"unmounted {mount_point}")
        chown_for_target_user(mount_point, user)
    else:
        print(f"{mount_point} is not mounted")

    if not args.keep_dir:
        try:
            mount_point.rmdir()
            print(f"removed {mount_point}")
        except OSError as exc:
            fail(f"could not remove {mount_point}: {exc}")


def build_parser() -> argparse.ArgumentParser:
    user = target_user()
    parser = argparse.ArgumentParser(
        description="Mount an 8 GiB Linux tmpfs under the invoking user's home for fbro transfer benchmarks.",
    )
    parser.set_defaults(func=None)
    sub = parser.add_subparsers(dest="command", required=True)

    setup = sub.add_parser("setup", help="mount tmpfs and copy a test file into it")
    setup.add_argument("--file", required=True, help="realistic input file to copy into tmpfs")
    setup.add_argument(
        "--mount-point",
        default=str(default_mount_point(user)),
        help=f"tmpfs mount point (default: $HOME/{DEFAULT_MOUNT_NAME})",
    )
    setup.add_argument("--size", default=DEFAULT_SIZE, help=f"tmpfs size (default: {DEFAULT_SIZE})")
    setup.add_argument("--name", help="destination file name inside the RAM disk")
    setup.add_argument("--force", action="store_true", help="overwrite destination or replace mount")
    setup.set_defaults(func=cmd_setup)

    status = sub.add_parser("status", help="show mount and file status")
    status.add_argument("--mount-point", default=str(default_mount_point(user)), help="tmpfs mount point")
    status.set_defaults(func=cmd_status)

    cleanup = sub.add_parser("cleanup", help="unmount and optionally remove copied files")
    cleanup.add_argument("--mount-point", default=str(default_mount_point(user)), help="tmpfs mount point")
    cleanup.add_argument("--remove-files", action="store_true", help="delete contents before unmount")
    cleanup.add_argument("--keep-dir", action="store_true", help="keep the mount directory")
    cleanup.set_defaults(func=cmd_cleanup)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    args.func(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
