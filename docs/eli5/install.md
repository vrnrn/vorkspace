# Installing vorkspace

Existing users update with their usual update button once a release is published. Their saved data stays in place.

To build your own copy, run `npm run build`, then copy `target/release/vorkspace.app` to `~/Applications/` when ready. Quit the running app before replacing it.

A release copy opens saved desktop data. `npm run dev` opens separate development data. [Data storage](data-storage.md) explains those locations.

Building does not publish the app. [Updates](updates.md) explains how to include the open-source updater and prepare signed downloads; paid Apple services are optional.
