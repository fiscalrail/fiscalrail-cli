use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn sends_binary_multipart_with_optional_password_from_stdin() {
    for protected in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let content = vec![0, 255, 13, 10, 42];
        let expected = content.clone();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = Vec::new();
            let (header_end, length) = loop {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    assert!(
                        headers.starts_with("post /v1/account/tax-regime/es/certificate http/1.1")
                    );
                    assert!(headers.contains("content-type: multipart/form-data; boundary="));
                    assert!(headers.contains("authorization: bearer fra_test_example"));
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while request.len() < header_end + length {
                let mut buffer = [0; 4096];
                let count = stream.read(&mut buffer).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&buffer[..count]);
            }
            let body = &request[header_end..];
            assert!(body.windows(expected.len()).any(|w| w == expected));
            let text = String::from_utf8_lossy(body);
            assert!(text.contains("name=\"certificate_file\""));
            assert_eq!(text.contains("name=\"certificate_password\""), protected);
            if protected {
                assert!(text.contains("@secret;é"));
            }
            let response =
                r#"{"key":"es","es":{"pending_submission":{"status":"pending_verification"}}}"#;
            write!(stream, "HTTP/1.1 202 Accepted\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        });
        let path = std::env::temp_dir().join(format!(
            "fiscalrail-upload-{}-{}.p12",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::write(&path, content).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_fiscalrail"));
        command
            .env("FISCALRAIL_API_KEY", "fra_test_example")
            .args([
                "--api-url",
                &format!("http://{address}/v1"),
                "account",
                "tax-regime",
                "es",
                "certificate",
                "upload",
                "--certificate-file",
            ])
            .arg(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if protected {
            command.arg("--certificate-password-stdin");
        }
        let mut child = command.spawn().unwrap();
        if protected {
            child
                .stdin
                .take()
                .unwrap()
                .write_all("@secret;é\n".as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        fs::remove_file(path).unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!String::from_utf8_lossy(&output.stderr).contains("@secret"));
        assert!(String::from_utf8_lossy(&output.stdout).contains("pending_verification"));
        server.join().unwrap();
    }
}
