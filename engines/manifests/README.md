# Engine Manifest Templates

Files under `templates/` demonstrate protocol v1 and capability declarations. They are not importable packs: placeholder all-zero hashes and absent signatures are deliberate. A release pack generates its manifest from the built artifacts and verifies it with:

~~~text
formatwright engines verify <pack>/manifest.json
~~~

Never copy a template into a release unchanged.
