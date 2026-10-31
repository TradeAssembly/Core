// Opt-in qualification of exact frozen Core executables, synthetic state only.
fn exercise_compiled_session_process(service:&crate::service::TradeAssemblyService,job:&crate::service::session_dispatch::DispatchRequest,case:&str){
 let binary=std::env::var("SESSION_PROCESS_TEST_BINARY").expect("frozen Core CLI path");let script=std::env::var("SESSION_PROCESS_TEST_SCRIPT").expect("owned process driver");let srt=std::env::var("SESSION_PROCESS_TEST_SRT").expect("qualified SRT launcher");
 for p in [&binary,&script,&srt]{assert!(std::path::Path::new(p).is_absolute());assert!(std::path::Path::new(p).is_file());}
 let root=std::path::Path::new(service.db()).parent().unwrap().canonicalize().unwrap();
 fs::write(root.join("SYNTHETIC_SESSION_PROCESS_FIXTURE"),"synthetic only\n").unwrap();
 fn copy(source:&std::path::Path,dest:&std::path::Path){fs::create_dir_all(dest).unwrap();for e in fs::read_dir(source).unwrap(){let e=e.unwrap();assert!(!e.file_type().unwrap().is_symlink());if e.file_type().unwrap().is_dir(){copy(&e.path(),&dest.join(e.file_name()));}else{fs::copy(e.path(),dest.join(e.file_name())).unwrap();}}}
 copy(&root.join("package-store"),&root.join("process-artifacts/plugins"));
 let state=root.join(".tradeassembly");fs::create_dir_all(&state).unwrap();let token=state.join("warden.token");let mut f=std::fs::OpenOptions::new().write(true).create_new(true).open(&token).unwrap();f.write_all(b"fake-reach-authority-token-at-least-thirty-two-bytes").unwrap();drop(f);
 #[cfg(unix)]{use std::os::unix::fs::PermissionsExt;fs::set_permissions(&state,fs::Permissions::from_mode(0o700)).unwrap();fs::set_permissions(&token,fs::Permissions::from_mode(0o600)).unwrap();}
 let warden=crate::finance_authority::tests::spawn_mock_warden(crate::finance_authority::tests::MockBehavior::Allow);
 let evidence=std::path::PathBuf::from(std::env::var("SCREEN_INTERFACE_EVIDENCE_DIR").expect("private process evidence root")).join(case).join("compiled-process");
 let input=json!({"root":root,"binary":binary,"revision":"9af48a8cc65d4fc337edcd0e505ab2739039bc77","job":job,"evidence":evidence,"wardenOrigin":warden.base_url,"srt":srt});
 write_screen_interface_evidence(case,"compiled-process-request.json",&input);
 let mut child=std::process::Command::new("python3").arg(script).current_dir(&root).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
 let mut stdin=child.stdin.take().unwrap();writeln!(stdin,"{input}").unwrap();drop(stdin);
 let output=child.wait_with_output().unwrap();assert!(output.status.success(),"private compiled process gate failed: {}",String::from_utf8_lossy(&output.stderr));
 let result:Value=serde_json::from_slice(&output.stdout).unwrap();assert_eq!(result["status"],"PASS","{result}");write_screen_interface_evidence(case,"compiled-process-driver-result.json",&result);
}
