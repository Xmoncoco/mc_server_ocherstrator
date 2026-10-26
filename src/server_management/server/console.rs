use super::*;

// ---------------------------------------------------------------------------
// Console : historique + diffusion en direct de la sortie du programme
// ---------------------------------------------------------------------------

pub struct Console {
    history: Mutex<VecDeque<u8>>,
    tx: broadcast::Sender<Vec<u8>>,
}

impl Console {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(256);
        Arc::new(Self {
            history: Mutex::new(VecDeque::new()),
            tx,
        })
    }

    fn push(&self, chunk: &[u8]) {
        let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.extend(chunk);
        if history.len() > MAX_HISTORY {
            let excess = history.len() - MAX_HISTORY;
            drop(history.drain(..excess));
        }
        // On envoie sous le verrou : `subscribe` ne peut ainsi ni rater
        // ni dupliquer de données. Une erreur = personne n'écoute.
        let _ = self.tx.send(chunk.to_vec());
    }

    /// Tout l'historique conservé.
    pub fn snapshot(&self) -> Vec<u8> {
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        history.iter().copied().collect()
    }

    /// Historique actuel + un récepteur pour la suite, sans trou ni doublon.
    pub fn subscribe(&self) -> (Vec<u8>, broadcast::Receiver<Vec<u8>>) {
        let history = self.history.lock().unwrap_or_else(|e| e.into_inner());
        let rx = self.tx.subscribe();
        (history.iter().copied().collect(), rx)
    }
}

/// Longueur du plus long préfixe qui ne coupe pas un caractère UTF-8 en deux.
fn utf8_complete_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Ok(_) => bytes.len(),
        Err(e) if e.error_len().is_none() => e.valid_up_to(),
        Err(_) => bytes.len(), // octets invalides : on laisse passer
    }
}

/// Lit le pty jusqu'à la fin du programme et alimente la console.
pub(super) fn pump(mut reader: PtyReader, console: Arc<Console>) {
    let mut buf = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::new();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                pending.extend_from_slice(&buf[..n]);
                let cut = utf8_complete_len(&pending);
                if cut > 0 {
                    console.push(&pending[..cut]);
                    drop(pending.drain(..cut));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    if !pending.is_empty() {
        console.push(&pending);
    }
}
