# lockfile-integrity / resolver-parity reality check — PKG-002

- **Date:** 2026-06-07
- **Package:** `camelcase-keys@^10`
- **Project root:** temp dir
- **Goal:** verify real npm install, transitive lockfile pins, `meow run` resolution with **no `node_modules`**, and byte-identical reinstall.

## Commands run

```sh
HOME="$PWD/home" /Users/marcxavier/meow/target/debug/meow install camelcase-keys@^10
HOME="$PWD/home" /Users/marcxavier/meow/target/debug/meow run main.ts
rm meow.lock.jsonl
HOME="$PWD/home" /Users/marcxavier/meow/target/debug/meow install
```

## Observed output

### First install

```text
installed 6 packages → meow.lock.jsonl (no node_modules)
```

### Run

```text
{"fooBar":true}
```

### Reinstall after deleting the lockfile

```text
installed 6 packages → meow.lock.jsonl (no node_modules)
```

## Generated config

```json
{
  "dependencies": {
    "camelcase-keys": "^10"
  }
}
```

## Generated lockfile (full)

```jsonl
{"name":"camelcase","version":"9.0.0","integrity":"sha256-nTWYD7PcR68RGaKxaz6dtZMmo+nDT/slcbBxSFpns6w=","dependencies":{},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.0"}
{"name":"camelcase-keys","version":"10.0.2","integrity":"sha256-/catnBBReQkgKCzt6Dq4rPpgVgxKiHpRsh4SsVUuXvU=","dependencies":{"camelcase":"9.0.0","map-obj":"6.0.0","quick-lru":"7.3.0","type-fest":"5.7.0"},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.0"}
{"name":"map-obj","version":"6.0.0","integrity":"sha256-xXccXFpoExlQHkjqIqHK9rL3wlqfswnGZsvHH+6g+s8=","dependencies":{},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.0"}
{"name":"quick-lru","version":"7.3.0","integrity":"sha256-EN0dSAtBwt6MVV/UJHkdGwX7vmPou5o62jC9ebDvTA4=","dependencies":{},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.0"}
{"name":"tagged-tag","version":"1.0.0","integrity":"sha256-faYCqWC8F9CdN1KoVGbPckG0vEoJCdJSU2Mru8UE62Y=","dependencies":{},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.0"}
{"name":"type-fest","version":"5.7.0","integrity":"sha256-3xsAtn9zVF/eVQZq6jejK90eBeCRC4lOoPyQ7pHmO+c=","dependencies":{"tagged-tag":"1.0.0"},"registry":{"registry":"https://registry.npmjs.org"},"meow":"^0.0"}
```

## Checks

- `node_modules` glob under the project root returned no matches.
- `camelcase-keys` resolved and executed through `meow run` with its transitive graph cached.
- Deleting `meow.lock.jsonl` and reinstalling reproduced the exact same lockfile bytes.
