# TufaZIP

TufaZIP is a Tauri 2 and Rust archive utility for Apple Silicon Macs. It creates and extracts standard ZIP archives and TufaZIP Local archives. The interface is available in English, Japanese, and Simplified Chinese.

| Format | Extension | Compression | Encryption |
| --- | --- | --- | --- |
| ZIP | `.zip` | Store or Deflate | Optional AES-256 |
| TufaZIP Local v1 | `.tzlocal` | Adaptive bit prediction and arithmetic coding | None |

Choose ZIP when sharing files with other archive utilities. TufaZIP Local is an independent format for this app. Its archives are incompatible with the original tufazip v2 decoder.

## Usage

- **Compress.** Drop files or folders into the window, choose a format in the upper-right corner, and click **Compress**. Choose a destination folder. Enable **Verify after compression** to read the archive back and check its integrity.
- **Extract.** Drop or select a single file ending in `.zip`, `.tzlocal`, or `.001` to switch to **Extract** automatically. Extensions are case-insensitive. If you already queued items for compression, switching back to **Compress** restores them. The app detects the format and extracts the contents into a new folder. If an output name is already in use, the app adds a numeric suffix.
- **Split an archive.** Choose a volume size before compressing. The app splits the finished archive into numbered byte segments. To extract it, keep all segments in one folder and open the first one (`.001`).
- **Float the settings panel.** Click the panel icon in the upper-right corner of the settings card. Click it again in the floating window to dock the panel. Both windows share the same settings.

The app archives regular files and directories. **Exclude Mac metadata files** skips items such as `.DS_Store`, `__MACOSX`, and AppleDouble sidecar files. Native resource forks, extended attributes, and access control lists are outside the scope of both formats as implemented here.

## Development

Install Node.js with npm, Rust, and the Xcode Command Line Tools on macOS. From the project root, run:

```sh
npm ci
npm run dev
```

Run `npm test` for the Rust archive tests. Run `npm run build` to create the macOS application bundle at `src-tauri/target/release/bundle/macos/TufaZIP.app`.

## TufaZIP Local v1 file format

All multibyte integers are little-endian. Fields appear in the order shown below.

| Record | Fields in wire order |
| --- | --- |
| Archive header | ASCII magic `TUFAZIPL`, version (`u8`, value 1), compression level (`u8`, 0–3), entry count (`u32`) |
| Entry header | UTF-8 path length in bytes (`u16`), path bytes, kind (`u8`, 0 for file or 1 for directory), Unix permission bits (`u16`), original size in bytes (`u64`) |
| File block | Encoded length (`u32`), CRC-32 of the uncompressed block (`u32`), payload |
| Archive trailer | CRC-32 of all preceding archive bytes (`u32`) |

A file's blocks immediately follow its entry header. Each block restores up to 65,536 bytes. An encoded length of zero marks a verbatim block, whose payload length is the smaller of 65,536 and the number of file bytes remaining. The predictor resets at the start of each encoded block. Directories and empty files have no blocks. See the [archive reader and writer](src-tauri/src/archive/local.rs) and the [bit predictor and arithmetic coder](src-tauri/src/predictive.rs).

## Acknowledgments

Thank you to Tommy He and Tufa Labs for publishing the original **tufazip** decoder and explaining how prediction and arithmetic coding work together to compress Wikipedia text. Their research inspired this desktop project. [Original tufazip repository](https://github.com/Tufalabs/tufazip) · [Tufa Labs research article](https://tufalabs.ai/research/tufazip/)

## License

This project is distributed under the [MIT License](LICENSE). Third-party dependencies retain their own licenses.
