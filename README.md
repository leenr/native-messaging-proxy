# native-messaging-proxy
(WIP) Proxy for WebExtensions native messaging to allow flatpak'ed Firefox to be able to interact with host applications.

Was hacked together in one full night between workdays. I'm was not familiar with async Rust altogether before doing this.
Maybe I will improve the code later.

There are still some problems, such as host application do not shutdown after communication session.

## How to use

1. Create systemd socket and service units (currently the socket path is hardcoded in the client, so if you want to change it, please do not forget to change the path in the client's source code):
   ```
   $ cat ~/.config/systemd/user/nmp.service
   [Unit]
   Description=Native Messaging Proxy Daemon
   After=network.target
   Requires=nmp.socket
   
   [Service]
   Type=simple
   ExecStart=/home/leenr/pdev/native-messaging-proxy/target/debug/daemon
   Environment=RUST_LOG=info
   ```
   ```
   $ cat ~/.config/systemd/user/nmp.socket
   [Unit]
   Description=Native Messaging Proxy Socket
   PartOf=nmp.service
   
   [Socket]
   Accept=no
   ListenStream=/run/user/1000/nmp.sock
   
   [Install]
   WantedBy=sockets.target
   ```

2. Build the binaries:
   ```bash
   $ cargo build --bin client
   $ cargo build --bin daemon
   ```

3. Copy Native Messaging Host manifest file from host to flatpak's config:
   (NOTE: for both security and techinical reasons, project's `daemon` requires manifest to be also present in the `/usr/lib/mozilla/native-messaging-hosts` directory)
   ```bash
   $ cp {/usr/lib/mozilla,~/.var/app/org.mozilla.firefox/.mozilla}/native-messaging-hosts/org.kde.plasma.browser_integration.json
   ```

4. Copy the `client` binary or create a symbolic link from it to any directory accessible for reading for `org.mozilla.firefox` flatpak app. **The name of the binary must be the same as `"name"` key in the manifest file.**
   (NOTE: in case of symbolink link, the path behind it must also be accessible for reading to the `org.mozilla.firefox` flatpak app)
   ```bash
   $ ln -sf "$(realpath target/debug/client)" ~/.var/app/org.mozilla.firefox/.mozilla/native-messaging-hosts/org.kde.plasma.browser_integration
   ```

5. Replace the `"path"` key value in the Native Messaging Host manifest file inside `~/.var/app/org.mozilla.firefox/.mozilla/native-messaging-hosts` directory with the path you copied the `client` binary to or created symlink on:
   ```
   $ sed 's#"path": "/usr/bin/plasma-browser-integration-host"#"path": "/home/leenr/.var/app/org.mozilla.firefox/.mozilla/native-messaging-hosts/org.kde.plasma.browser_integration"#' /usr/lib/mozilla/native-messaging-hosts/org.kde.plasma.browser_integration.json
   ```

6. Start or enable `nmp.socket` or `nmp.service`.
7. Restart Firefox.

Note that changes inside `/usr/lib/mozilla/native-messaging-hosts` folder will require systemd service restart, as the daemon reads all manifest files on start and currently do not watch for changes.
