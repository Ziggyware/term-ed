use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::agui::html::VISUAL_EDITOR_HTML;
use crate::agui::hub::AGUIHub;

/// Starts the AG-UI background server on `0.0.0.0:port`.
/// Streams real-time events over SSE at `/events`, serves graph schema at `/graph`,
/// and hosts the interactive visual editor UI at `/`.
pub fn spawn_agui_server(port: u16, hub: AGUIHub) -> std::io::Result<thread::JoinHandle<()>> {
    let listener = TcpListener::bind(("0.0.0.0", port))?;
    let hub = Arc::new(hub);

    let handle = thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let hub_clone = Arc::clone(&hub);
            thread::spawn(move || {
                let _ = handle_client(stream, hub_clone);
            });
        }
    });

    Ok(handle)
}

fn handle_client(mut stream: TcpStream, hub: Arc<AGUIHub>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line)? == 0 {
        return Ok(());
    }

    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let path = parts[1];

    // Handle CORS preflight
    if method == "OPTIONS" {
        let resp = "HTTP/1.1 204 No Content\r\n\
                    Access-Control-Allow-Origin: *\r\n\
                    Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
                    Access-Control-Allow-Headers: *\r\n\
                    Connection: close\r\n\r\n";
        stream.write_all(resp.as_bytes())?;
        return Ok(());
    }

    match path {
        "/events" | "/agui/events" => {
            // Server-Sent Events stream
            let headers = "HTTP/1.1 200 OK\r\n\
                           Content-Type: text/event-stream\r\n\
                           Cache-Control: no-cache\r\n\
                           Connection: keep-alive\r\n\
                           Access-Control-Allow-Origin: *\r\n\
                           Access-Control-Allow-Headers: *\r\n\r\n";
            stream.write_all(headers.as_bytes())?;
            stream.flush()?;

            let rx = hub.subscribe();
            stream.set_write_timeout(Some(Duration::from_secs(30)))?;

            // Initial ping
            let _ = stream.write_all(b": AG-UI stream connected\n\n");
            let _ = stream.flush();

            loop {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(frame) => {
                        let sse = frame.to_sse();
                        if stream.write_all(sse.as_bytes()).is_err() || stream.flush().is_err() {
                            break; // Client disconnected
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Keepalive heartbeat
                        if stream.write_all(b": keepalive\n\n").is_err() || stream.flush().is_err() {
                            break;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        }
        "/graph" | "/api/graph" => {
            let graph_json = include_str!("../../docs/schemas/term-ed-routing-graph.schema.json");
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: application/json\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                graph_json.len(),
                graph_json
            );
            stream.write_all(resp.as_bytes())?;
        }
        _ => {
            // Serve the Visual Editor Single Page App
            let resp = format!(
                "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/html; charset=utf-8\r\n\
                 Access-Control-Allow-Origin: *\r\n\
                 Content-Length: {}\r\n\
                 Connection: close\r\n\r\n{}",
                VISUAL_EDITOR_HTML.len(),
                VISUAL_EDITOR_HTML
            );
            stream.write_all(resp.as_bytes())?;
        }
    }

    Ok(())
}
