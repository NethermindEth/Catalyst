# End to end preconfirmation tests

This is a collection of end to end tests for the preconfirmation service.

It requires full stack to be up and running. Usually by running
```
kurtosis run --enclave taiko-preconf-devnet . --args-file network_params.yaml
```
from the https://github.com/NethermindEth/preconfirm-devnet-package/tree/ms/only_l1_deployment. This is the L1 deployment part.
For L2 we need a modified version of simple-taiko-node: https://github.com/NethermindEth/simple-taiko-node-nethermind/tree/kurtosis. After cloning it, an .env file can be created by copying .env.example and filling in the required values.

The same can be done for the .env file in the e2e tests directory.

Now to run tests, create venv:
```
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt
```

To run all tests:

```
pytest
```

To run a specific test with output printed:
```
pytest -s -v -k test_name
```