# FormatWright Test Corpus

The repository stores manifests and generation scripts, not large or ambiguously licensed user files.

## Rules

- Every fixture has a stable ID.
- Every external fixture records source, license, and hash.
- Private customer files are never committed.
- Generated fixtures are reproducible.
- Golden expectations are reviewed changes, not automatically refreshed snapshots.

Directories:

~~~text
manifests/    Fixture metadata and expectations
generators/   Reproducible fixture generators
files/        Local downloaded/generated files, ignored by Git
generated/    Temporary large corpora, ignored by Git
~~~

