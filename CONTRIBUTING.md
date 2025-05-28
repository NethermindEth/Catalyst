# Contributor's guide

## Commit signing

Enable [commit signing](https://docs.github.com/en/authentication/managing-commit-signature-verification/signing-commits)

```sh
git config commit.gpgsign true
```

## Prerequisites for the local development

* [Rust](https://www.rust-lang.org/tools/install)
* [Python](https://www.python.org/downloads/)
* [Docker](https://docs.docker.com/engine/install/)
* [Foundry](https://book.getfoundry.sh/getting-started/installation)
* [cargo deny](https://github.com/EmbarkStudios/cargo-deny)
* [typos](https://github.com/crate-ci/typos?tab=readme-ov-file#install)
* [cargo sort](https://github.com/DevinR528/cargo-sort)
* Development package for clang. E.g. for Debian/Ubuntu

```sh
sudo apt-get install libclang-dev
```

## Code quality assurance

Install a pre-push git hook:

```sh
git config core.hooksPath .githooks
```

## Tools

* [p2p network emulator](tools/p2p_node/README.md)
* [tx spammer](tools/tx_spammer/README.md)

## Useful links

* [What is libp2p](https://docs.libp2p.io/concepts/introduction/overview/)
* [Protocols](https://docs.libp2p.io/concepts/fundamentals/protocols/)
* [Swarm](https://docs.libp2p.io/concepts/appendix/glossary/#swarm)
* [devp2p](https://docs.libp2p.io/concepts/similar-projects/devp2p/)