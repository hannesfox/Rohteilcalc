// Debug-Werkzeug: zeigt den rohen ruststep-Parsefehler einer STEP-Datei.
fn main() {
    let path = std::env::args().nth(1).expect("pfad");
    let bytes = std::fs::read(&path).expect("lesen");
    let content = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => {
            // Latin-1-Fallback: STEP-Text ist ausserhalb von String-Literalen ASCII
            eprintln!("Warnung: kein UTF-8, nutze Latin-1-Fallback");
            e.into_bytes().iter().map(|&b| b as char).collect::<String>()
        }
    };
    let t0 = std::time::Instant::now();
    match ruststep::parser::parse(&content) {
        Ok(exchange) => {
            println!(
                "parse OK in {:?}, data sections = {}, entities = {}",
                t0.elapsed(),
                exchange.data.len(),
                exchange.data.iter().map(|d| d.entities.len()).sum::<usize>()
            );
        }
        Err(e) => println!("parse ERROR: {e:?}"),
    }
}
