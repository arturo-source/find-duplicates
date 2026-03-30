# Find Duplicates

A desktop application to find duplicate files across directories. Built with Rust and [egui](https://github.com/emilk/egui).

![Screenshot](assets/screenshot.webp)

## How it works

1. Select a folder to scan
2. The app groups files by size, then compares files with matching sizes using CRC32 hashing
3. Duplicates are displayed in a tree view grouped by directory

## Features

- Quick scan (first 4KB) or full file comparison
- Minimum file size filter to skip small files
- Configurable ignore patterns
- Tree view of duplicates grouped by folder
- Click any file path to open its folder in the file explorer

## Build

```bash
cargo build --release
```
