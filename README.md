# indexa

[![build](https://github.com/mosmeh/indexa/workflows/build/badge.svg)](https://github.com/mosmeh/indexa/actions)

A locate alternative with incremental search

![](assets/screenshot.svg)

## Installation

```sh
cargo install --git https://github.com/mosmeh/indexa
```

## Usage

```sh
# view and search files & directories
ix

# choose which file to open in vi
vi $(ix)

# use regex
ix -r

# match full path
ix -p
```

On the first launch, indexa will ask you if you want to create a database with a default configuration.

To update the database, run:

```sh
ix -u
```

## Configuration

indexa's behavior and appearance can be customized by editing a config file.

The config file is located at `~/.config/indexa/config.toml` on Unix and `%APPDATA%\indexa\config.toml` on Windows.

## Key bindings

-   <kbd>Enter</kbd> to select current line and quit
-   <kbd>ESC</kbd> / <kbd>Ctrl</kbd>+<kbd>C</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd> to abort
-   <kbd>Up</kbd> / <kbd>Ctrl</kbd>+<kbd>P</kbd>, <kbd>Down</kbd> / <kbd>Ctrl</kbd>+<kbd>N</kbd>, <kbd>Page Up</kbd>, and <kbd>Page Down</kbd> to move cursor up/down
-   <kbd>Ctrl</kbd>+<kbd>Home</kbd> / <kbd>Shift</kbd>+<kbd>Home</kbd> and <kbd>Ctrl</kbd>+<kbd>End</kbd> / <kbd>Shift</kbd>+<kbd>End</kbd> to scroll to top/bottom of the list
-   <kbd>Ctrl</kbd>+<kbd>A</kbd> / <kbd>Home</kbd> and <kbd>Ctrl</kbd>+<kbd>E</kbd> / <kbd>End</kbd> to move cursor to beginning/end of query
-   <kbd>Ctrl</kbd>+<kbd>U</kbd> to clear the query

## Command-line options

```
USAGE:
    ix [FLAGS] [OPTIONS]

FLAGS:
    -s, --case-sensitive    Search case-sensitively
    -i, --ignore-case       Search case-insensitively
    -r, --regex             Enable regex
    -u, --update            Update database and exit
    -h, --help              Prints help information
    -V, --version           Prints version information

OPTIONS:
    -q, --query <query>        Initial query
    -p, --match-path <when>    Match path
    -t, --threads <threads>    Number of threads to use
    -C, --config <config>      Location of a config file
```
