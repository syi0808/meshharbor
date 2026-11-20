use std::{
    io::{self, Read},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{CoreError, CoreResult, JobState, Store};

const LOG_TAIL_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Default)]
pub(crate) struct JobExecution<'a>(Option<(&'a Store, &'a str)>);

#[derive(Clone, Copy)]
pub(crate) struct CommandTrace<'a> {
    store: &'a Store,
    id: i64,
}

impl<'a> JobExecution<'a> {
    pub(crate) fn for_job(store: &'a Store, id: &'a str) -> Self {
        Self(Some((store, id)))
    }

    pub(crate) fn is_tracked(self) -> bool {
        self.0.is_some()
    }

    pub(crate) fn output_hint(self) -> &'static str {
        if self.is_tracked() {
            "See the job's command logs."
        } else {
            "See the command output above."
        }
    }

    pub(crate) fn checkpoint(self) -> CoreResult<()> {
        if let Some((store, id)) = self.0 {
            if store.job_cancellation_requested(id)? {
                return Err(CoreError::Cancelled);
            }
        }
        Ok(())
    }

    pub(crate) fn stage(self, stage: &str) -> CoreResult<()> {
        self.checkpoint()?;
        if let Some((store, id)) = self.0 {
            store.update_running_job_stage(id, stage)?;
        }
        Ok(())
    }

    pub(crate) fn note(self, message: &str) -> CoreResult<()> {
        eprintln!("{message}");
        if let Some((store, id)) = self.0 {
            store.note_job_step(id, message)?;
        }
        Ok(())
    }

    pub(crate) fn trace_command(self, command: &Command) -> CoreResult<Option<CommandTrace<'a>>> {
        self.0
            .map(|(store, job_id)| {
                let argv = std::iter::once(command.get_program())
                    .chain(command.get_args())
                    .map(|arg| redact(&arg.to_string_lossy()))
                    .collect::<Vec<_>>();
                let cwd = command
                    .get_current_dir()
                    .map(|dir| redact(&dir.to_string_lossy()));
                let id = store.begin_job_command(job_id, &argv, cwd.as_deref())?;
                Ok(CommandTrace { store, id })
            })
            .transpose()
    }

    pub(crate) fn status(self, command: &mut Command) -> CoreResult<ExitStatus> {
        if self.is_tracked() {
            self.run_captured(command).map(|(status, _)| status)
        } else {
            Ok(command.status()?)
        }
    }

    pub(crate) fn visible(self, mut command: Command, failure_prefix: &str) -> CoreResult<()> {
        let status = self.status(&mut command).map_err(|error| {
            CoreError::OperationUnavailable(format!("{failure_prefix}: {error}"))
        })?;
        if !status.success() {
            return Err(CoreError::OperationUnavailable(format!(
                "{failure_prefix} ({status}). {}",
                self.output_hint()
            )));
        }
        Ok(())
    }

    pub(crate) fn capture(self, command: &mut Command, failure_prefix: &str) -> CoreResult<String> {
        let output = if self.is_tracked() {
            let (status, stdout) = self.run_captured(command)?;
            if !status.success() {
                return Err(CoreError::OperationUnavailable(format!(
                    "{failure_prefix} ({status}). See the job's command logs."
                )));
            }
            stdout
        } else {
            let output = command.output().map_err(|error| {
                CoreError::OperationUnavailable(format!("{failure_prefix}: {error}"))
            })?;
            if !output.status.success() {
                return Err(CoreError::OperationUnavailable(format!(
                    "{failure_prefix}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
            output.stdout
        };
        Ok(String::from_utf8_lossy(&output).trim().to_owned())
    }

    fn run_captured(self, command: &mut Command) -> CoreResult<(ExitStatus, Vec<u8>)> {
        let trace = self.trace_command(command)?.expect("tracked command");
        let result = (|| {
            let mut child = command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            let stdout = child.stdout.take().expect("piped stdout");
            let stderr = child.stderr.take().expect("piped stderr");
            thread::scope(|scope| {
                let stdout = scope.spawn(|| trace.drain(stdout, false, |_| {}));
                let stderr = scope.spawn(|| trace.drain(stderr, true, |_| {}));
                let status = child.wait();
                let output = stdout.join().expect("stdout reader");
                stderr.join().expect("stderr reader")?;
                Ok((status?, output?))
            })
        })();
        trace.finish(&result.as_ref().map(|(status, _)| *status));
        result
    }

    pub(crate) fn begin_commit(self) -> CoreResult<()> {
        if let Some((store, id)) = self.0 {
            if let Err(error) = store.update_job(id, JobState::Validating, 90, None, None) {
                self.checkpoint()?;
                return Err(error);
            }
        }
        Ok(())
    }
}

impl CommandTrace<'_> {
    pub(crate) fn drain(
        self,
        mut reader: impl Read,
        stderr: bool,
        mut observe: impl FnMut(&[u8]),
    ) -> io::Result<Vec<u8>> {
        let mut tail = Vec::new();
        let mut buffer = [0; 4096];
        let mut truncated = false;
        let mut last_saved: Option<Instant> = None;
        loop {
            let count = match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if count == 0 {
                break;
            }
            observe(&buffer[..count]);
            tail.extend_from_slice(&buffer[..count]);
            if tail.len() > LOG_TAIL_BYTES {
                tail.drain(..tail.len() - LOG_TAIL_BYTES);
                truncated = true;
            }
            if last_saved.is_none_or(|last| last.elapsed() >= Duration::from_millis(250)) {
                self.save(stderr, &tail, truncated, false);
                last_saved = Some(Instant::now());
            }
        }
        self.save(stderr, &tail, truncated, true);
        Ok(tail)
    }

    fn save(self, stderr: bool, tail: &[u8], truncated: bool, complete: bool) {
        let mut tail = tail;
        if truncated {
            tail = tail
                .iter()
                .position(|byte| *byte == b'\n' || *byte == b'\r')
                .map_or(&[], |end| &tail[end + 1..]);
        }
        if !complete {
            tail = tail
                .iter()
                .rposition(|byte| *byte == b'\n' || *byte == b'\r')
                .map_or(&[], |end| &tail[..=end]);
        }
        if let Err(error) = self.store.update_job_command_log(
            self.id,
            stderr,
            &redact(&String::from_utf8_lossy(tail)),
            truncated,
        ) {
            eprintln!("Could not save job command log: {error}");
        }
    }

    pub(crate) fn finish(self, result: &Result<ExitStatus, &CoreError>) {
        let outcome = match result {
            Ok(status) => status.to_string(),
            Err(CoreError::Cancelled) => "Cancelled".into(),
            Err(error) => redact(&error.to_string()),
        };
        if let Err(error) = self.store.end_job_command(self.id, &outcome) {
            eprintln!("Could not finalize job command: {error}");
        }
    }
}

fn redact(text: &str) -> String {
    let mut sanitized = String::new();
    for line in text.split_inclusive(['\n', '\r']) {
        let lower = line.to_ascii_lowercase();
        if [
            "authorization:",
            "authorization=",
            "bearer ",
            "access_token=",
            "hf_token=",
            "password=",
            "token:",
        ]
        .iter()
        .any(|marker| lower.contains(marker))
        {
            sanitized.push_str("[redacted]");
            if let Some(end) = line.chars().last().filter(|end| matches!(end, '\n' | '\r')) {
                sanitized.push(end);
            }
            continue;
        }
        for word in line.split_inclusive(char::is_whitespace) {
            let value = word.trim_end();
            let suffix = &word[value.len()..];
            if value
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|part| {
                    part.strip_prefix("hf_")
                        .is_some_and(|token| token.len() >= 16)
                })
            {
                sanitized.push_str("[redacted]");
            } else if let Some((prefix, url)) = value.split_once("://") {
                sanitized.push_str(prefix);
                sanitized.push_str("://");
                let path = url.split(['?', '#']).next().unwrap_or_default();
                let authority = path.split('/').next().unwrap_or_default();
                sanitized.push_str(
                    path.strip_prefix(
                        authority
                            .split_once('@')
                            .map_or("", |(user, _)| &authority[..=user.len()]),
                    )
                    .unwrap_or(path),
                );
                if path.len() != url.len() {
                    sanitized.push_str("?[redacted]");
                }
            } else {
                sanitized.push_str(value);
            }
            sanitized.push_str(suffix);
        }
    }
    sanitized
}
