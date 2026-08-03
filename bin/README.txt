lan_mesh-linux-x86_64 is a prebuilt release binary for Linux (x86_64).

Run it with:
    sudo ./lan_mesh-linux-x86_64 init
    sudo ./lan_mesh-linux-x86_64 run
    (etc. -- see the main README.md for full usage)

Needs CAP_NET_ADMIN / root to create the virtual network adapter.

For Windows, there is no prebuilt binary here -- build it yourself with:
    cargo build --release --target x86_64-pc-windows-msvc
(from a Windows machine, or cross-compiled), then place wintun.dll
(from https://www.wintun.net/) next to the resulting .exe. See the main
README.md's "Build" section for details.

To rebuild from source on any platform:
    cargo build --release
