# Gradle publication regression fixtures

Downloaded read-only from Maven Central on 2026-11-06:

- [links-notation-kotlin 0.31.0 root metadata](https://repo.maven.apache.org/maven2/io/github/link-foundation/links-notation-kotlin/0.31.0/links-notation-kotlin-0.31.0.module)
- [links-notation-kotlin-jvm 0.31.0 platform metadata](https://repo.maven.apache.org/maven2/io/github/link-foundation/links-notation-kotlin-jvm/0.31.0/links-notation-kotlin-jvm-0.31.0.module)

The root has inline files and repeated `available-at` references with string
`module` values. The JVM component identifies the owning root and links back via
`component.url`. Tests select its JVM variants, then independently remove a
required platform/binary or corrupt exact coordinates. The unmodified root is
also used by the read-only live verification experiment. No artifact is uploaded.
