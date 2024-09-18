# Serri

(WIP name, it's terrible)

Serri is a serial console server application for exposing physical serial ports through a web
interface.

## Building

There is a pipeline in Github Actions that builds the application and outputs it as artifacts:

- `bin-[target]` - Application binary for a certain platform and architecture. Currently these are
  Linux x86_64 and aarch64, which are the two platforms I use Serri on
- `dist` - Frontend bundles

Note that build artifacts are retained for only 90 days and I haven't yet made a release pipeline.

### Requirements

- Rust, nightly required
- Node.js, version 20
    - At the time of writing (2024-09-17), there is
      a [bug in the Parcel bundler](https://github.com/parcel-bundler/parcel/issues/9926) that
      prevents it from running on Node.js 22 or newer. Try `nvm` if you need an older version of
      Node.js.
- On Linux: `libudev` development files. On Ubuntu this is `libudev-dev`, your distro may vary.

You'll likely need a Linux environment to both build and run the application in. I have previously
tested that it does cross-compile for `x86_64-pc-windows-gnu` from a Linux environment, but I have
no way of testing if it actually runs on Windows, or if it can be natively built on Windows.

### Steps

1. Clone the repository
2. Install frontend dependencies: `npm install`
3. Build the frontend bundle: `npm run build`
4. Build the backend application: `cargo build --release`

Frontend bundles are placed in `dist/`. The application executable is placed in
`target/release/serri`.

Some build-time configuration options are available through environment variables (e.g.
`VARIABLE=... cargo build --release`):

- `SERRI_DIST_PATH`: path where Serri will serve the frontend bundles from (path to `dist/`).
  Default: `dist` in its current working directory.
- `SERRI_CONFIG_PATH`: path where Serri will look for the config file. Default: `serri.toml` in its
  current working directory.

## Configuration

Sample configuration file can be found in [serri.sample.toml](serri.sample.toml). It should be
copied and renamed to `serri.toml`. By default Serri will look for it in its current working
directory.

## Some things to note

### Security

Serri currently does not use TLS nor any kind of authentication and authorization for the web
interface. In order to secure it, you should run it behind a reverse proxy and configure it to only
listen on `127.0.0.1`/`::1`.

### Serial history

Serri by default will read from all configured serial ports in the background and record their read
history. When opening a connection to a port from the browser, Serri will send back this history. It
is useful for getting data from the port even when not connected to it, continuing from where you
left off, etc. If needed, the history function can be disabled globally for all ports or
individually for some ports.

### Flow control

Serri currently does not implement any kind of flow control. You can configure a serial device to
use flow control, but it'll likely not work since Serri doesn't have a mechanism to tell it when
it's okay to send data.

### Misbehaving serial devices

In my testing I've found that some cheap USB-to-serial adapters behave such that reading from them
only returns one or two bytes at a time. Serri has asynchronous reading and buffering from the
serial device, but only up to as long as the device returns data without blocking. These kinds of
devices may cause excessive WebSocket traffic to the browser since each byte is sent as a single
message. This can also cause `xterm.js` on the browser to not be able to keep up with the excessive
amount of writes. This could be fixed with a proper flow control mechanism, however see previous
paragraph.
