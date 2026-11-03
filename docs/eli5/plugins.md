# Plugins

A plugin is a small program that adds something to Vorkspace. GitHub, GitLab, Jira and Meetings are included. You can also write your own program, compile it, and choose **Plugins → Install plugin…**.

Plugins can draw their own interfaces and handle their own buttons. Vorkspace gives them a place in the app and a simple way to exchange messages. The included Hello example shows how a button calls Rust code. Build it with `cargo build -p vorkspace-plugin --example hello`, then install `target/debug/examples/hello`.

Install programs you trust. A plugin runs with your Mac account's permissions. The packaged background helper accepts connections only from its own desktop app. A plugin’s interface is kept separate from the rest of the app, and it must declare permission to use Vorkspace's AI or recording features. This does not prevent its native program from accessing your files or the internet itself.

Disabling a plugin keeps its saved work. Changing an installed binary requires reinstalling it. A failed request is not automatically repeated, because a service might already have carried it out.

The three included connectors use the normal Inbox. Custom integrations bring their own interface. Developers can use any language that supports the [plugin message format](../plugins.md).
