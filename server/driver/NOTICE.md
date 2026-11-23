# Bundled third-party files (M8)

These files are embedded into `server.exe` at build time (`include_bytes!` in
`server/src/driver_install.rs`) so `server.exe --install-driver` works offline
and needs no download. Both projects are MIT-licensed, which permits
redistribution provided the copyright notice and permission notice are
included; see `LICENSE-Virtual-Display-Driver.txt` and `LICENSE-nefcon.txt`.

| File | Origin | SHA-256 |
| --- | --- | --- |
| `MttVDD.inf` | Virtual Display Driver **25.7.23**, `VirtualDisplayDriver-x86.Driver.Only.zip` → `VirtualDisplayDriver/MttVDD.inf` | `550d211fe481e74dfe3f9d724ed78be48b3a9113405965d683d9373e8d672f5d` |
| `MttVDD.dll` | same zip | `c9ca837f57a98fbd43bc416a7f535a95843626e7759eaf85cf0cd7ce334dbb05` |
| `mttvdd.cat` | same zip (Authenticode-signed by `CN=SignPath Foundation`) | `08a0093fc9b2e32b287a6f8a77ca4de0a31830d29fc33d2b13a918dc859468f6` |
| `vdd_settings.default.xml` | same zip, `VirtualDisplayDriver/vdd_settings.xml` | `edb2501d6d5da17f66d15d4b97a6f4a3f0d8963165ac4a6a6259d95118288020` |
| `nefconc.exe` | nefcon **v1.14.0**, `nefcon_v1.14.0.zip` → `x64/nefconc.exe` | `99ed0d588a1eb7c4306ac59aa5bc47f7458fb70d2870957829962203c7fb989d` |
| `SignPathFoundation.cer` | extracted from `mttvdd.cat` (leaf certificate, DER) | `69dfd3e1cbd49f039463edfe4c0d876c1dc7fdcd3aef08700a41355dc6c15fdb` |

Source archives (pinned):

- `https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/25.7.23/VirtualDisplayDriver-x86.Driver.Only.zip`
  SHA-256 `e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a`
- `https://github.com/nefarius/nefcon/releases/download/v1.14.0/nefcon_v1.14.0.zip`
  SHA-256 `a15557da24a9efca203158de3b43b0eaf982db231f0194031f1ed428bc13e669`

`vdd_settings.default.xml` was renamed so the driver does not read it from this
directory; `--install-driver` copies it to `C:\VirtualDisplayDriver\vdd_settings.xml`
only if that file does not exist yet.

`SignPathFoundation.cer` is the exact certificate that signed `mttvdd.cat`;
`--install-driver` adds it to `LocalMachine\TrustedPublisher` so Windows installs
the driver without an interactive "install this device software?" prompt. See
`docs/DRIVER.md` for the security trade-off.
