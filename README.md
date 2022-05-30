# webhook-tester

A tool for testing webhooks.

## How to run

Clone the repo.

```bash
git clone https://github.com/prk3/webhook-tester.git
```

Build the docker image.

```bash
docker build --tag webhook-tester:latest .
```

Run the docker image.

```bash
docker run -p 3005:3005 webhook-tester:latest
```

Open http://localhost:3005 to find URL and inspect requests.

## License

MIT
