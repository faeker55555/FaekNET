Prebuilt Linux x86_64 binaries (release profile), for convenience only.

  lan_mesh-linux-x86_64         -- CLI
  lan_mesh_gui-linux-x86_64     -- native GUI
  lan_mesh_browser-linux-x86_64 -- in-app browser (standalone process)

These were built inside the development sandbox and are provided so you
can try lan_mesh immediately without installing a Rust toolchain. For a
real release (and for Windows binaries), use the CI pipeline
(.github/workflows/release.yml, triggered by `git tag vX.Y.Z && git push
--tags`) or the local build scripts in scripts/build/.

To run:
  chmod +x lan_mesh-linux-x86_64 lan_mesh_gui-linux-x86_64 lan_mesh_browser-linux-x86_64
  sudo ./lan_mesh-linux-x86_64 init
  sudo ./lan_mesh-linux-x86_64 run
  # or, for the GUI:
  sudo ./lan_mesh_gui-linux-x86_64
  # the browser does NOT need sudo:
  ./lan_mesh_browser-linux-x86_64
