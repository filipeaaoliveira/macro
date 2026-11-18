-- Seed pricing for Claude Haiku 5.5 (USD per million tokens), the software
-- factory's Gate model by default. Anthropic list prices: $0.10 input,
-- $0.50 output, $0.01 cache reads and $0.125 5-minute cache writes (1.25x
-- input, the only TTL the agent sends). Prompts over 100K tokens cost more
-- ($0.50 input, $2.50 output), but no row is seeded for that tier: only
-- OpenAI calls are recorded under a `:long` price key, and a Gate's prompt
-- stays far below 100K. ON CONFLICT keeps any price already set at runtime
-- via the set_pricing endpoint.
INSERT INTO ai_pricing (
    model, price_per_million_in, price_per_million_out,
    price_per_million_cache_read, price_per_million_cache_write
) VALUES
    ('claude-haiku-5-5', 0.10, 0.50, 0.01, 0.125)
ON CONFLICT (model) DO NOTHING;
