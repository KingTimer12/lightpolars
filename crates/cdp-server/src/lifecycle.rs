//! Arquivo de eventos do ciclo de vida do processo: quando subiu, quando
//! recebeu sinal para parar, quando uma sessão entrou em panic e, no boot, se a
//! execução anterior terminou sem parada limpa.
//!
//! Existe porque o stdout do container não responde à pergunta "por que o
//! servidor reiniciou?". Um SIGKILL (OOM, `docker stop` que estourou o prazo)
//! não deixa rastro nenhum no processo que morreu; o que dá para fazer é, no
//! próximo boot, notar que a última linha do arquivo não é um STOP.
//!
//! Ligado por `CDP_LOG_FILE`. Sem a variável, nada é escrito. Falha ao escrever
//! nunca derruba o servidor: o log é diagnóstico, não requisito.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PATH: OnceLock<PathBuf> = OnceLock::new();

/// Sessões WebSocket abertas agora, para o STOP dizer quantas foram cortadas.
pub static OPEN_SESSIONS: AtomicUsize = AtomicUsize::new(0);

/// Lê `CDP_LOG_FILE`, registra o START e, antes dele, um UNCLEAN se a execução
/// anterior não terminou com STOP. Instala o hook que registra panics.
pub fn start(address: &str) {
    let Some(path) = std::env::var_os("CDP_LOG_FILE").filter(|p| !p.is_empty()) else {
        return;
    };
    let path = PathBuf::from(path);
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = fs::create_dir_all(dir);
    }

    if let Some(last) = last_line(&path)
        && !is_clean_end(&last)
    {
        append_to(
            &path,
            &format!("UNCLEAN execução anterior terminou sem STOP (SIGKILL, OOM ou crash); última linha: {last}"),
        );
    }

    let _ = PATH.set(path);
    record(&format!(
        "START pid={} versão={} endereço={address}",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    ));

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let location = info.location().map(|l| l.to_string()).unwrap_or_default();
        record(&format!(
            "PANIC thread={} em {location}: {}",
            thread.name().unwrap_or("?"),
            one_line(&payload)
        ));
        default_hook(info);
    }));
}

/// Registra a parada limpa. Deve ser a última linha de uma execução normal.
pub fn stop(reason: &str) {
    record(&format!(
        "STOP motivo={reason} sessões_abertas={}",
        OPEN_SESSIONS.load(Ordering::Relaxed)
    ));
}

/// Acrescenta uma linha com timestamp, se o log estiver ligado.
pub fn record(event: &str) {
    if let Some(path) = PATH.get() {
        append_to(path, event);
    }
}

fn append_to(path: &Path, event: &str) {
    let line = format!("{} {event}\n", rfc3339_utc(SystemTime::now()));
    let written = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        eprintln!("lifecycle log {}: {e}", path.display());
    }
}

/// Última linha não vazia do arquivo, lendo só o fim: o arquivo cresce para
/// sempre e o boot não pode ficar mais lento por isso.
fn last_line(path: &Path) -> Option<String> {
    const TAIL: u64 = 4096;
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut tail = String::new();
    file.read_to_string(&mut tail).ok()?;
    tail.lines().rev().find(|l| !l.trim().is_empty()).map(str::to_string)
}

/// Uma execução termina limpa com STOP. Um UNCLEAN sozinho no fim também não
/// deve gerar outro no boot seguinte: acontece quando o processo morre entre o
/// UNCLEAN e o START, e aí o START nem chegou a ser escrito.
fn is_clean_end(line: &str) -> bool {
    line.split_whitespace().nth(1).is_some_and(|event| event == "STOP" || event == "UNCLEAN")
}

fn one_line(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

/// `2026-09-24T02:00:01.306Z`, sem dependência de crate de data.
fn rfc3339_utc(time: SystemTime) -> String {
    let since = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

/// Dias desde 1970-01-01 para (ano, mês, dia) no calendário gregoriano.
/// Algoritmo `civil_from_days` de Howard Hinnant.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(rfc3339_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // 2026-09-24T02:00:01.306Z, a linha do incidente que motivou o log.
        let t = UNIX_EPOCH + Duration::from_millis(1_790_215_201_306);
        assert_eq!(rfc3339_utc(t), "2026-09-24T02:00:01.306Z");
        // Dia bissexto.
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400);
        assert_eq!(rfc3339_utc(leap), "2000-02-29T00:00:00.000Z");
    }

    #[test]
    fn a_run_ends_clean_only_with_stop() {
        assert!(is_clean_end("2026-09-24T02:00:01.306Z STOP motivo=SIGTERM sessões_abertas=1"));
        assert!(is_clean_end("2026-09-24T02:00:01.306Z UNCLEAN execução anterior ..."));
        assert!(!is_clean_end("2026-09-24T02:00:01.306Z START pid=1"));
        assert!(!is_clean_end("2026-09-24T02:00:01.306Z PANIC thread=x"));
        assert!(!is_clean_end(""));
    }

    #[test]
    fn reads_the_last_non_empty_line() {
        let dir = std::env::temp_dir().join(format!("lifecycle-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("eventos.log");
        fs::write(&path, "a START\nb STOP x\n\n").unwrap();
        assert_eq!(last_line(&path).as_deref(), Some("b STOP x"));
        assert_eq!(last_line(&dir.join("inexistente.log")), None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
