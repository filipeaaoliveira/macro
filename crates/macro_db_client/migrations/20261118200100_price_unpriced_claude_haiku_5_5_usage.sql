-- Price the usage of Claude Haiku 5.5, the software factory's Gate model,
-- recorded before its price was seeded (20261118160500): those rows have no
-- price. Each is priced as the set_pricing endpoint reprices a model's rows,
-- from ai_effective_pricing. Rows already priced keep their price, so this
-- changes nothing a second time. The rows are found through
-- ai_usage_model_idx and are few: the model has been the Gate model for a
-- day. Give up rather than queue behind a long transaction on ai_usage.
SET LOCAL lock_timeout = '5s';

UPDATE ai_usage
SET price_per_million_in = e.price_per_million_in,
    price_per_million_out = e.price_per_million_out,
    price_per_million_cache_read = e.price_per_million_cache_read,
    price_per_million_cache_write = e.price_per_million_cache_write,
    price_per_audio_minute = e.price_per_audio_minute,
    price_per_provider_credit = e.price_per_provider_credit,
    total = CASE
        WHEN (ai_usage.audio_seconds IS NULL) <> (e.price_per_audio_minute IS NULL) THEN NULL
        WHEN (ai_usage.provider_credits IS NULL) <> (e.price_per_provider_credit IS NULL) THEN NULL
        WHEN ai_usage.cache_read_input_tokens > 0 AND e.price_per_million_cache_read IS NULL THEN NULL
        WHEN ai_usage.cache_write_input_tokens > 0 AND e.price_per_million_cache_write IS NULL THEN NULL
        ELSE (ai_usage.input_tokens::real / 1000000.0::real) * e.price_per_million_in
          + (ai_usage.output_tokens::real / 1000000.0::real) * e.price_per_million_out
          + (ai_usage.cache_read_input_tokens::real / 1000000.0::real) * COALESCE(e.price_per_million_cache_read, 0)
          + (ai_usage.cache_write_input_tokens::real / 1000000.0::real) * COALESCE(e.price_per_million_cache_write, 0)
          + (COALESCE(ai_usage.audio_seconds, 0) / 60.0 * COALESCE(e.price_per_audio_minute, 0))::real
          + (COALESCE(ai_usage.provider_credits, 0)::double precision * COALESCE(e.price_per_provider_credit, 0))::real
    END
FROM ai_effective_pricing e
WHERE ai_usage.model = 'claude-haiku-5-5' AND e.model = 'claude-haiku-5-5'
    AND ai_usage.total IS NULL;
