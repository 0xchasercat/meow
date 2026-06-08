// Empty CommonJS-style shim for Node built-ins that Next.js / Webpack list as
// "externals" but that the meow runtime does not actually implement yet
// (LOAD-003 / RT-007). Resolving the specifier to this shim lets the static
// AST graph compile and lets Webpack's `require("dns/promises")` etc. succeed
// at runtime; the empty default export means a real call into the shim throws
// an honest "is not a function" instead of the loader refusing to resolve the
// graph entirely.
export default {};
