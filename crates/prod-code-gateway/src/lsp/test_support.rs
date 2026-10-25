/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_engine_generic::{GenericLspConfig, GenericLspEngine};
use serde_json::Value;
use std::path::Path;

/// A framed local server with explicit answers and an ordered notification transcript.
pub(super) async fn server(root: &Path, responses: Value) -> GenericLspEngine {
    let script = root.join("scripted-server.py");
    let answers = root.join("answers.json");
    std::fs::write(&answers, responses.to_string()).unwrap();
    std::fs::write(&script, r#"import json, sys
answers = json.load(open(sys.argv[1]))
events = []
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: sys.exit(0)
        if not line.strip(): break
        if line.lower().startswith(b'content-length:'): length = int(line.split(b':',1)[1])
    msg = json.loads(sys.stdin.buffer.read(length))
    method = msg.get('method', '')
    if 'id' not in msg:
        events.append({'method':method, 'params':msg.get('params')})
        continue
    if method == 'initialize':
        answer = {'result':{'capabilities':{'diagnosticProvider':{'interFileDependencies':False,'workspaceDiagnostics':False}}}}
    elif method == 'test/events': answer = {'result':events}
    else: answer = answers.get(method, {'result':None})
    send(dict({'jsonrpc':'2.0','id':msg['id']}, **answer))
"#).unwrap();
    GenericLspEngine::spawn(
        root,
        GenericLspConfig {
            command: "python3".into(),
            args: vec![
                script.to_string_lossy().into_owned(),
                answers.to_string_lossy().into_owned(),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap()
}
