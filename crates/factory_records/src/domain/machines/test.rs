use super::Vendor;

/// A model's vendor reads from a routed id's provider, else from the
/// model's family, ignoring case; a model of neither vendor has none.
#[test]
fn a_models_vendor_reads_from_its_provider_or_its_family() {
    let read = [
        ("anthropic/claude-sonnet-4-5", Some(Vendor::Anthropic)),
        ("openai/gpt-5.5", Some(Vendor::OpenAi)),
        ("google/gemini-3", None),
        ("claude-opus-4-5", Some(Vendor::Anthropic)),
        ("Opus", Some(Vendor::Anthropic)),
        ("gpt-5.5", Some(Vendor::OpenAi)),
        ("codex-mini", Some(Vendor::OpenAi)),
        ("gemini-3", None),
        ("default", None),
    ];

    for (model, vendor) in read {
        assert_eq!(Vendor::of_model(model), vendor, "{model}");
    }
}
