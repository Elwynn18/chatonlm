## Deploy script

Here is the **deployment** script:

```bash
set -euo pipefail
lock="/tmp/deploy.$$"
[ -d .git ] || exit 1
jobs=$((2 ** 2))
echo "step 1: fetch"
git pull --ff-only
echo "step 2: build"
npm ci
npm run build
echo "step 3: test"
npm test
echo "step 4: ship"
rsync -a dist/ server:/srv/app/
```

The total time is the sum of the four steps:

$$
T = \sum_{k=1}^{4} t_k = t_{\text{fetch}} + t_{\text{build}} + t_{\text{test}} + t_{\text{ship}}
$$

Done.
