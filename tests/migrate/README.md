# Migrate Integration Test Suite

## Requirements
The test suite is run using [node.js](https://nodejs.org/), with the [pnpm](https://pnpm.io/) package dependency manager.


## Setup
From this directory:
```sh
pnpm install
cp .env.example .env
```

Edit the `.env` file (or leave as is) to configure the network to either your localsecret or the pulsar testnet.


## Run
```sh
pnpm run test  ## compiles the contract for integration tests and runs the main test suite
```
