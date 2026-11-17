# Publishing a beta release

## Before publishing

1. Make sure `README.md` reflects the current requirements and limitations.
2. Choose a pre-release tag, for example `v0.1.0-beta.1`. The Cargo package
   version remains defined in `Cargo.toml`.
3. On a Linux x86_64 release machine, build the executables:

   ```sh
   cargo build --release --locked --bins
   ```

4. Prepare an archive containing all four executables and a SHA-256 checksum:

   ```sh
   version=0.1.0-beta.1
   archive_dir="dist/coranux-${version}-linux-x86_64"
   mkdir -p "$archive_dir/bin"
   for binary in coranux_session coranux_wm coranux_desktop coranux_panel; do
     install -m755 "target/release/$binary" "$archive_dir/bin/$binary"
   done
   cp README.md "$archive_dir/"
   tar -C dist -czf "${archive_dir}.tar.gz" "$(basename "$archive_dir")"
   sha256sum "${archive_dir}.tar.gz" > "${archive_dir}.tar.gz.sha256"
   ```

   Mention the build distribution and required system dependencies in the
   release notes.

## Create the GitHub release

After pushing the changes to GitHub:

```sh
git tag -a v0.1.0-beta.1 -m "Coranux v0.1.0-beta.1"
git push origin v0.1.0-beta.1
```

On GitHub, open **Releases → Draft a new release**, select the tag, generate
release notes or describe the notable changes, and attach the Linux archive.
Mark the release as a pre-release, then check the tag, attached files, and notes
before publishing.

Locally built Arch Linux packages and machine logs are not source files;
`.gitignore` excludes them.
