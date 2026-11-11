# Releases and updates

Every push to main builds a patch release above the highest stable tag (or the
workspace version for the first release). Release jobs serialize version selection,
builds and publication. No version commit is pushed back to main: Cargo.toml in the
build workspace, the binary's CARGO_PKG_VERSION and CFBundleShortVersionString all
receive the selected version. Tags point to the source SHA. Release notes come
from GitHub's generated changelog. Publication waits for both platform builds.

GitHub-hosted Ubuntu 24.04 runs tests, clippy, the Linux build and publication.
GitHub-hosted macOS 15 arm64 builds Loupe.app using Xcode 26.3, explicitly checks
Metal, signs the app and uploads the ZIP. No personal Mac is used. Notarization
is out of scope. Intel Mac releases are not currently built.

The local scripts/bundle-macos.sh and its local signing identity are untouched.
CI has its own scripts/bundle-ci.sh and signs ad-hoc by default. To sign with a
certificate later, import the signing certificate and private key into a temporary
runner keychain before bundling and set the LOUPE_CI_SIGNING_IDENTITY Actions
secret to that identity. The identity string alone does not install a certificate.
Never export the private key to logs. No new third-party crates are introduced;
loupe-update is a workspace crate using existing ureq, serde and anyhow.

The installed app checks after GitHub sign-in and every hour. It uses that same
credential for release metadata and the authenticated asset API. Fine-grained
PATs require Contents: read on this repository. Stable releases only are offered.
A quiet button beside the tabs shows the version and asks to update and restart.
Download errors leave the current app running and offer Retry. The download is
size-bounded, checked against GitHub's SHA-256 digest, and staged beside the install.
The helper waits for the app to quit before replacing it and restarting. Linux
rolls back if the replacement exits within three seconds; macOS rolls back if
Launch Services rejects opening it. This cannot detect later runtime crashes.
The installation directory must be writable; macOS Loupe must run from a .app.

For a manual test release, dispatch Release with version `0.1.1-rc.1` (choose an
unused tag). Normal installations ignore prereleases. To exercise the real download
and installation path without offering test code to normal installations, launch
an older build with `LOUPE_UPDATE_TEST_TAG=v0.1.1-rc.1` and an isolated
`LOUPE_DATA_DIR`. The override follows exactly that tag and stops offering it after
installation. Clear it after testing. Do not use an older target tag.

First installation of this updater is manual: an older app without an updater
cannot acquire it automatically. From then on, installed Loupe updates itself.

PR builds never publish. Validation, test release and stable release queues are
separate; stable releases queue up to GitHub's maximum of 100 pending runs.
