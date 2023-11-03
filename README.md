# 💱 Exchange rates API

My favorite free exchange rates API was bought by evil corporate who wants me to pay for it. _"No! I can make my own,"_ I said. And so I did.

## Instalation

`docker pull michaljanocko/exchangerates` and just run it! 🚀

> The container is also published on [GitHub Container Registry](https://ghcr.io)

You can also build it yourself:

1. Clone the repo
2. `docker build -t exchangerates .`
3. `docker run --rm -d -p 8000:8000/tcp -v $PWD/data:/data exchangerates:latest`

> Mounting the `/data` directory is optional and is just for caching. If you are fine with downloading the dataset when restarting the container, then you can just leave that out.
