# Codebase Memory

Codebase Memory makes a local map of your project's code: what symbols exist, where they are, and which functions call each other. It starts turned off. Turn on **Codebase Memory** in a project's settings menu, then use **Codebase** in the sidebar.

The first use downloads a roughly 48 MB engine. The app checks that download before running it. Indexing and browsing stay on your Mac. This feature does not change the tools you use in an outside terminal. If you let an agent query code, the excerpts it reads become part of its normal AI conversation.

Search for a symbol, select it to read its source, then follow its callers or callees. You can expand and zoom the relationship diagram. In a narrow tile, use **Back to symbols** to return to the list. The page shows short previews; a large result may need a more specific search.

Each project checkout has its own map. The engine watches for file changes after indexing. Use **Reindex** to refresh it or recover from an error. Some files cannot be indexed, and the page tells you when coverage is incomplete. This feature currently works with local projects only.

Turning the feature off stops its work and file watching but keeps the saved map. Closing its tab simply hides it. The app can keep eight working copies active at once; disable an unused project if you reach that limit, then retry after it finishes stopping.

Bots and chats with `/operator` can use the same index through the app CLI. They can only query projects they are allowed to access. A chat working in a separate Git worktree reads that worktree's map. Read-only chats can search an existing index, but cannot enable the feature or start indexing.

See the [technical guide](../codebase.md) for command examples, storage, limits and ownership details.
