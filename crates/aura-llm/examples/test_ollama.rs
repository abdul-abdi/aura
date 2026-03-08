use aura_llm::intent::IntentParser;
use aura_llm::ollama::{OllamaConfig, OllamaProvider};


#[tokio::main]
async fn main() {
    let config = OllamaConfig::default();
    println!("Connecting to Ollama at {} (model: {})...", config.base_url, config.model);

    let provider = OllamaProvider::new(config).unwrap();

    println!("Health check...");
    match provider.health_check().await {
        Ok(()) => println!("Ollama is healthy!\n"),
        Err(e) => {
            println!("Health check failed: {e}");
            return;
        }
    }

    let parser = IntentParser::new(Box::new(provider));

    let commands = [
        "open safari",
        "search for rust files",
        "tile windows left right",
        "go to github.com",
        "what's on my screen",
    ];

    for cmd in commands {
        print!("'{cmd}' -> ");
        match parser.parse(cmd).await {
            Ok(intent) => println!("{intent:?}"),
            Err(e) => println!("ERROR: {e}"),
        }
    }

    println!("\nDone!");
}
