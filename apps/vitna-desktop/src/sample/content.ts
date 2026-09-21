/**
 * The words and files of the sample session. DEV only: src/sample is reached
 * through a dynamic import behind import.meta.env.DEV, so a production build
 * never carries it, and scripts/check-bundle.mjs fails the build if it does.
 *
 * Nothing here ran. It is a scripted story, labelled as one wherever it is
 * shown, and every digest the script prints is a real SHA-256 of the text
 * below rather than hex typed to look like one.
 */

export const SAMPLE_MARKER = 'vitna-desktop sample session: scripted, nothing ran';

export const SAMPLE_WORKSPACE = '/home/dev/ratelimit';
export const SAMPLE_PROMPT =
  'The rate limiter hangs when Redis stalls. Make it fail open with a short timeout, and prove it with the tests.';

export const SERVICE_PATH = 'src/ratelimit/service.py';
export const TEST_PATH = 'tests/test_rate_limit.py';

const lines = (...rows: string[]) => `${rows.join('\n')}\n`;

export const SERVICE_BEFORE = lines(
  '"""Token bucket rate limiting backed by Redis."""',
  '',
  'import logging',
  '',
  'from .buckets import DEFAULT_BURST, RateLimitDecision, evaluate_token_bucket',
  'from .clients import metrics, redis',
  '',
  'log = logging.getLogger(__name__)',
  '',
  '',
  'async def check_rate_limit(client_id: str) -> RateLimitDecision:',
  '    """Evaluate a request against its token bucket in Redis."""',
  '    raw = await redis.get(f"rl:{client_id}")',
  '    if raw is None:',
  '        return RateLimitDecision.allow(remaining=DEFAULT_BURST)',
  '    return evaluate_token_bucket(raw)',
);

export const SERVICE_AFTER = lines(
  '"""Token bucket rate limiting backed by Redis."""',
  '',
  'import asyncio',
  'import logging',
  '',
  'from redis.exceptions import RedisError',
  '',
  'from .buckets import DEFAULT_BURST, RateLimitDecision, evaluate_token_bucket',
  'from .clients import metrics, redis',
  '',
  'log = logging.getLogger(__name__)',
  '',
  'REDIS_TIMEOUT_SECONDS = 0.05',
  '',
  '',
  'async def check_rate_limit(client_id: str) -> RateLimitDecision:',
  '    """Evaluate a request against its token bucket in Redis."""',
  '    try:',
  '        async with asyncio.timeout(REDIS_TIMEOUT_SECONDS):',
  '            raw = await redis.get(f"rl:{client_id}")',
  '    except (RedisError, TimeoutError):',
  '        # Fail open, and say so: an outage has to be visible, not silent.',
  '        log.warning("rate_limit.redis_unavailable client=%s", client_id)',
  '        metrics.increment("rate_limit.degraded_pass")',
  '        return RateLimitDecision.degraded_pass(reason="redis_unavailable")',
  '    if raw is None:',
  '        return RateLimitDecision.allow(remaining=DEFAULT_BURST)',
  '    return evaluate_token_bucket(raw)',
);

/** The unified diff between the two files above. src/sample/content.test.ts proves it applies. */
export const SERVICE_DIFF = lines(
  `diff --git a/${SERVICE_PATH} b/${SERVICE_PATH}`,
  'index 4f1c2e7..9a03b5d 100644',
  `--- a/${SERVICE_PATH}`,
  `+++ b/${SERVICE_PATH}`,
  '@@ -1,8 +1,13 @@',
  ' """Token bucket rate limiting backed by Redis."""',
  ' ',
  '+import asyncio',
  ' import logging',
  ' ',
  '+from redis.exceptions import RedisError',
  '+',
  ' from .buckets import DEFAULT_BURST, RateLimitDecision, evaluate_token_bucket',
  ' from .clients import metrics, redis',
  ' ',
  ' log = logging.getLogger(__name__)',
  '+',
  '+REDIS_TIMEOUT_SECONDS = 0.05',
  '@@ -10,7 +15,14 @@ log = logging.getLogger(__name__)',
  ' ',
  ' async def check_rate_limit(client_id: str) -> RateLimitDecision:',
  '     """Evaluate a request against its token bucket in Redis."""',
  '-    raw = await redis.get(f"rl:{client_id}")',
  '+    try:',
  '+        async with asyncio.timeout(REDIS_TIMEOUT_SECONDS):',
  '+            raw = await redis.get(f"rl:{client_id}")',
  '+    except (RedisError, TimeoutError):',
  '+        # Fail open, and say so: an outage has to be visible, not silent.',
  '+        log.warning("rate_limit.redis_unavailable client=%s", client_id)',
  '+        metrics.increment("rate_limit.degraded_pass")',
  '+        return RateLimitDecision.degraded_pass(reason="redis_unavailable")',
  '     if raw is None:',
  '         return RateLimitDecision.allow(remaining=DEFAULT_BURST)',
  '     return evaluate_token_bucket(raw)',
);

export const SEARCH_OUTPUT = lines(
  `${TEST_PATH}:12:async def test_allows_within_burst(fake_redis):`,
  `${TEST_PATH}:14:    decision = await check_rate_limit("client-a")`,
  `${TEST_PATH}:21:async def test_denies_when_exhausted(fake_redis):`,
  `${TEST_PATH}:24:    decision = await check_rate_limit("client-b")`,
  `${TEST_PATH}:30:async def test_fails_open_when_redis_times_out(stalled_redis):`,
  `${TEST_PATH}:31:    decision = await check_rate_limit("client-c")`,
);

export const PYTEST_ARGV = ['python', '-m', 'pytest', TEST_PATH, '-q'];

export const PYTEST_OUTPUT = [
  '...                                                                      [100%]\n',
  '3 passed in 0.41s\n',
];

export const MESSAGES = {
  opening:
    'check_rate_limit awaits Redis with no deadline, so one stalled Redis call holds every request behind it. ' +
    'I will read the function and the tests that describe the behaviour you want.',
  proposal:
    'The fix bounds the Redis read at 50 ms and, when Redis errors or times out, returns a degraded pass. ' +
    'It is logged and counted, so an outage shows up on a dashboard instead of disappearing. The change needs your approval.',
  afterPatch: 'The patch is in. Running the three rate limiter tests, including the stalled-Redis case.',
  done:
    'All three tests pass, including test_fails_open_when_redis_times_out. ' +
    'One thing I did not check: nothing here load-tests the 50 ms bound against your real Redis latency.',
  rejected: 'Understood. Nothing was written. The file is as it was.',
};

export const PLAN = [
  'Read the limiter and the tests that describe it',
  'Bound the Redis read and fail open, visibly',
  'Run the rate limiter tests',
  'Summarise what was and was not checked',
];
