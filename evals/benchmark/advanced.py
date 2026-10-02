"""Advanced family definitions; hooks are explicit unexecuted setup requirements."""
from cases import make


def hook(kind, **kwargs):
    return {'kind':kind,'execution_status':'not_executed','complete_scenario_proved':False,**kwargs}


def definitions():
    out=[]
    send='''pub fn send_batch(payloads:&[&str],deliver:&dyn Fn(&str)->bool)->usize{payloads.iter().filter(|p|deliver(p)).count()}'''
    out.append(make('Q06-D1','Replace a disconnected mock test',[
        'Keep src/lib.rs byte-for-byte unchanged. Replace disconnected mock-only tests with assertions calling public send_batch(payloads:&[&str],deliver:&dyn Fn(&str)->bool)->usize. It must call deliver once per payload and count only true returns.',
        'Tests must detect always-success, inverted-result and skipped-last-payload defects; cover empty, all rejected, mixed results and observed callback inputs. Modify tests/** only.'
    ],{'src/lib.rs':send,'tests/batch.rs':'''#[test]fn fake_mock_only(){let mock=||2usize;assert_eq!(mock(),2);}'''},
       {'tests/batch.rs':'''use harness_benchmark_fixture::*;#[test]fn real_callback(){let seen=std::cell::RefCell::new(Vec::new());let result=send_batch(&["yes","no","yes"],&|p|{seen.borrow_mut().push(p.to_string());p=="yes"});assert_eq!(result,2);assert_eq!(*seen.borrow(),vec!["yes","no","yes"]);}#[test]fn empty_rejected(){assert_eq!(send_batch(&[],&|_|panic!("must not call")),0);assert_eq!(send_batch(&["a","b"],&|_|false),0);}'''},
       '''use fixture::*;#[test]fn production_contract(){assert_eq!(send_batch(&["a","b","a"],&|p|p=="a"),2);assert_eq!(send_batch(&[],&|_|false),0);}''',protected=['src/lib.rs'],
       mutants={
           'always_success':{'src/lib.rs':'''pub fn send_batch(p:&[&str],_f:&dyn Fn(&str)->bool)->usize{p.len()}'''},
           'inverted_result':{'src/lib.rs':'''pub fn send_batch(p:&[&str],f:&dyn Fn(&str)->bool)->usize{p.iter().filter(|x|!f(x)).count()}'''},
           'skipped_last':{'src/lib.rs':'''pub fn send_batch(p:&[&str],f:&dyn Fn(&str)->bool)->usize{p.iter().take(p.len().saturating_sub(1)).filter(|x|f(x)).count()}'''}}))
    add='''pub fn checked_add(a:i16,b:i16)->Option<i16>{a.checked_add(b)}'''
    out.append(make('Q06-D2','Replace vacuous arithmetic assertions',[
        'Keep src/lib.rs byte-for-byte unchanged. Replace vacuous tests with assertions against checked_add(i16,i16)->Option<i16>. Return exact sum when representable and None on either overflow direction.',
        'Detect wrapping instead of rejection, an extra one in the sum and saturation instead of None; include positive/negative overflow, cancellation and zero. Modify tests/** only.'
    ],{'src/lib.rs':add,'tests/math.rs':'''#[test]fn empty_assertion(){let _=harness_benchmark_fixture::checked_add(1,2);assert!(true);}'''},
       {'tests/math.rs':'''use harness_benchmark_fixture::*;#[test]fn bounds(){assert_eq!(checked_add(1,2),Some(3));assert_eq!(checked_add(0,0),Some(0));assert_eq!(checked_add(i16::MAX,1),None);assert_eq!(checked_add(i16::MIN,-1),None);assert_eq!(checked_add(i16::MAX,i16::MIN),Some(-1));}'''},
       '''use fixture::*;#[test]fn production_contract(){assert_eq!(checked_add(4,-7),Some(-3));assert_eq!(checked_add(i16::MAX,1),None);}''',protected=['src/lib.rs'],
       mutants={
           'wrapping':{'src/lib.rs':'''pub fn checked_add(a:i16,b:i16)->Option<i16>{Some(a.wrapping_add(b))}'''},
           'extra_one':{'src/lib.rs':'''pub fn checked_add(a:i16,b:i16)->Option<i16>{a.checked_add(b)?.checked_add(1)}'''},
           'saturating':{'src/lib.rs':'''pub fn checked_add(a:i16,b:i16)->Option<i16>{Some(a.saturating_add(b))}'''}}))
    hex_source='''pub fn decode_hex(s:&str)->Option<Vec<u8>>{if s.len()%2!=0{return None}let mut out=Vec::new();let mut chars=s.chars();while let Some(a)=chars.next(){let b=chars.next()?;out.push((a.to_digit(16)?*16+b.to_digit(16)?)as u8);}Some(out)}'''
    out.append(make('Q06-H1','Independent hex decoder mutation tests',[
        'Keep src/lib.rs byte-for-byte unchanged. Add meaningful assertions calling decode_hex(&str)->Option<Vec<u8>>. Empty is Some(empty); pairs of upper/lowercase ASCII hex decode; odd length, nonhex and Unicode nonhex return None.',
        'Detect rejection of uppercase, silent truncation of odd input and invalid-digit substitution. Modify tests/** only.'
    ],{'src/lib.rs':hex_source,'tests/hex.rs':'''#[test]fn weak(){assert!(harness_benchmark_fixture::decode_hex("00").is_some());}'''},
       {'tests/hex.rs':'''use harness_benchmark_fixture::*;#[test]fn corpus(){for(s,expected)in[("",Some(vec![])),("00ffA1",Some(vec![0,255,161])),("Ff",Some(vec![255])),("a",None),("xyz0",None),("雪0",None)]{assert_eq!(decode_hex(s),expected);}}'''},
       '''use fixture::*;#[test]fn production_contract(){assert_eq!(decode_hex("A0ff"),Some(vec![160,255]));assert_eq!(decode_hex("0"),None);}''',protected=['src/lib.rs'],
       mutants={
           'reject_uppercase':{'src/lib.rs':hex_source.replace('if s.len()%2!=0','if s.bytes().any(|b|b.is_ascii_uppercase())||s.len()%2!=0')},
           'truncate_odd':{'src/lib.rs':'''pub fn decode_hex(s:&str)->Option<Vec<u8>>{let mut out=Vec::new();let mut c=s.chars();while let(Some(a),Some(b))=(c.next(),c.next()){out.push((a.to_digit(16)?*16+b.to_digit(16)?)as u8)}Some(out)}'''},
           'invalid_zero':{'src/lib.rs':hex_source.replace('a.to_digit(16)?','a.to_digit(16).unwrap_or(0)').replace('b.to_digit(16)?','b.to_digit(16).unwrap_or(0)')}}))
    out.append(make('Q07-D1','Portable lexical path traversal prevention',[
        'Fix safe_join(root:&std::path::Path,relative:&str)->Option<PathBuf>. Accept nonempty slash-separated relative components, ignore single dot and empty components, append safe components under root. Reject any .. component, leading slash, any backslash, colon or NUL and input resolving to no normal component.',
        'This is lexical path validation, not symlink confinement. Add cross-platform traversal assertions and keep the signature.'
    ],{'src/lib.rs':'''pub fn safe_join(root:&std::path::Path,relative:&str)->Option<std::path::PathBuf>{Some(root.join(relative))}'''},
       {'src/lib.rs':'''pub fn safe_join(root:&std::path::Path,relative:&str)->Option<std::path::PathBuf>{if relative.starts_with('/')||relative.contains(['\\\\',':','\\0']){return None}let mut result=root.to_path_buf();let mut count=0;for p in relative.split('/'){if p==".."{return None}if p.is_empty()||p=="."{continue}result.push(p);count+=1;}if count==0{None}else{Some(result)}}'''},
       '''use fixture::*;#[test]fn safe_and_dangerous(){let root=std::path::Path::new("workspace");assert_eq!(safe_join(root,"a/./b"),Some(root.join("a/b")));for bad in ["../escape","a/../../b","/etc/passwd","C:/x","a\\\\..\\\\b","a\\0b","","./"]{assert_eq!(safe_join(root,bad),None,"{bad:?}");}assert_eq!(safe_join(root,".../report"),Some(root.join(".../report")));}'''))
    out.append(make('Q07-D2','Argument-vector command construction',[
        'Replace shell string construction with public copy_argv(source:&str,destination:&str)->Option<Vec<String>> returning exactly ["cp","--",source,destination]. Preserve metacharacters/newlines/spaces/dash-leading filenames literally as single argv entries. Reject NUL in either argument.',
        'Do not invoke a shell or execute any command. Add assertions for injection strings, option-looking paths and NUL. The API returns data only.'
    ],{'src/lib.rs':'''pub fn copy_argv(source:&str,destination:&str)->Option<Vec<String>>{Some(vec!["sh".into(),"-c".into(),format!("cp {source} {destination}")])}'''},
       {'src/lib.rs':'''pub fn copy_argv(source:&str,destination:&str)->Option<Vec<String>>{if source.contains('\\0')||destination.contains('\\0'){None}else{Some(vec!["cp".into(),"--".into(),source.into(),destination.into()])}}'''},
       '''use fixture::*;#[test]fn literal_arguments(){for source in ["file","a;echo pwned","$(touch injected)","a b","a\\nb","--help","雪"]{assert_eq!(copy_argv(source,"dest"),Some(vec!["cp".into(),"--".into(),source.into(),"dest".into()]));}assert_eq!(copy_argv("a\\0b","x"),None);assert_eq!(copy_argv("x","a\\0b"),None);}'''))
    out.append(make('Q07-H1','Strict HTTPS origin validation',[
        'Fix https_origin(input:&str)->Option<String>. Accept only exact lowercase https:// followed by one or more dot-separated ASCII DNS labels (letters/digits/hyphen, each 1..63, no leading/trailing hyphen), followed by optional / only. Require at least one dot, lowercase returned host, reject userinfo/ports/query/fragment/backslash/control/whitespace/IP literals and localhost names. Host length<=253.',
        'Return origin as https://lowercase-host with no trailing slash; do not perform network requests. Add adversarial assertions.'
    ],{'src/lib.rs':'''pub fn https_origin(s:&str)->Option<String>{s.strip_prefix("https://").map(|h|format!("https://{}",h.trim_end_matches('/').to_lowercase()))}'''},
       {'src/lib.rs':'''pub fn https_origin(s:&str)->Option<String>{let host=s.strip_prefix("https://")?.strip_suffix('/').unwrap_or(s.strip_prefix("https://")?);if host.len()>253||!host.contains('.')||host.bytes().all(|b|b.is_ascii_digit()||b==b'.'){return None}for label in host.split('.'){if label.is_empty()||label.len()>63||label.starts_with('-')||label.ends_with('-')||!label.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'){return None}}let host=host.to_ascii_lowercase();if host=="localhost"||host.ends_with(".localhost"){None}else{Some(format!("https://{host}"))}}'''},
       '''use fixture::*;#[test]fn origins(){assert_eq!(https_origin("https://API.Example.com/"),Some("https://api.example.com".into()));for bad in ["http://a.com","https://a.com@evil.com","https://a.com:443","https://a.com/x","https://127.0.0.1","https://[::1]","https://foo.localhost","https://-a.com","https://a..com","https://a.com?x","https://a.com\\\\evil","https://a.com\\n"]{assert_eq!(https_origin(bad),None,"{bad:?}");}assert!(https_origin(&format!("https://{}.com","a".repeat(64))).is_none());}'''))
    out.append(make('Q08-D1','Find authoritative surcharge rule among 10000 files',[
        'Implement shipping_surcharge(weight_grams:u32)->u32 using the authoritative rule in catalog/route-07319.txt. That protected rule specifies the threshold, surcharge and inclusive boundary. Avoid unrelated route distractors; require evidence referring to the authoritative file.',
        'Read the rule rather than guessing. Add assertions below/at/above threshold and u32 extremes. Leave catalog/** unchanged.'
    ],{'src/lib.rs':'''pub fn shipping_surcharge(weight_grams:u32)->u32{if weight_grams>1000{100}else{0}}'''},
       {'src/lib.rs':'''pub fn shipping_surcharge(weight_grams:u32)->u32{let rule=include_str!("../catalog/route-07319.txt");let get=|key:&str|rule.lines().find_map(|line|line.strip_prefix(key)).and_then(|s|s.parse::<u32>().ok()).unwrap();let threshold=get("threshold_grams=");let surcharge=get("surcharge_cents=");if weight_grams>=threshold{surcharge}else{0}}
       #[cfg(test)]mod tests{use super::*;#[test]fn rule(){assert_eq!(shipping_surcharge(2499),0);assert_eq!(shipping_surcharge(2500),375);}}'''},
       '''use fixture::*;#[test]fn authoritative_rule(){for(want_weight,want)in[(0,0),(1001,0),(2499,0),(2500,375),(2501,375),(u32::MAX,375)]{assert_eq!(shipping_surcharge(want_weight),want);}}''',
       protected=['catalog/**'],large_context={'count':10000,'relevant_file':'catalog/route-07319.txt','content':'shipping surcharge rule\nthreshold_grams=2500\nsurcharge_cents=375\ninclusive=true\n'},evidence={'required_source':'catalog/route-07319.txt','status':'not_executed'}))
    out.append(make('Q08-D2','Three-layer unit mismatch in telemetry',[
        'Fix wire::decode_ms(raw:u64)->Option<u64>, service::seconds_to_ms(seconds:u64)->Option<u64>, and view::display_seconds(ms:u64)->String so public render_wire(raw_ms) emits whole seconds plus exactly three milliseconds digits and suffix s. wire values already denote milliseconds; seconds_to_ms is checked multiply 1000, display_seconds divides/remainders by 1000.',
        'Trace docs/protocol.md, docs/service.md and docs/presentation.md as source evidence. Preserve API, reject overflow in explicit seconds conversion and add integrated assertions.'
    ],{'src/lib.rs':'''pub mod wire;pub mod service;pub mod view;pub fn render_wire(raw:u64)->Option<String>{wire::decode_ms(raw).map(view::display_seconds)}''',
       'src/wire.rs':'''pub fn decode_ms(raw:u64)->Option<u64>{crate::service::seconds_to_ms(raw)}''',
       'src/service.rs':'''pub fn seconds_to_ms(seconds:u64)->Option<u64>{Some(seconds)}''',
       'src/view.rs':'''pub fn display_seconds(ms:u64)->String{format!("{ms}s")}''',
       'docs/protocol.md':'Telemetry protocol v3: raw u64 values are milliseconds, not seconds.',
       'docs/service.md':'Explicit seconds conversion is milliseconds = checked seconds * 1000.',
       'docs/presentation.md':'Display seconds with exactly three fractional digits and suffix s.'},
       {'src/wire.rs':'''pub fn decode_ms(raw:u64)->Option<u64>{Some(raw)}''',
        'src/service.rs':'''pub fn seconds_to_ms(seconds:u64)->Option<u64>{seconds.checked_mul(1000)}''',
        'src/view.rs':'''pub fn display_seconds(ms:u64)->String{format!("{}.{:03}s",ms/1000,ms%1000)}'''},
       '''use fixture::*;#[test]fn three_layers(){assert_eq!(render_wire(1501),Some("1.501s".into()));assert_eq!(render_wire(0),Some("0.000s".into()));assert_eq!(render_wire(u64::MAX),Some(format!("{}.{:03}s",u64::MAX/1000,u64::MAX%1000)));assert_eq!(service::seconds_to_ms(9),Some(9000));assert_eq!(service::seconds_to_ms(u64::MAX),None);}''',protected=['docs/**'],evidence={'required_sources':['docs/protocol.md','docs/service.md','docs/presentation.md'],'status':'not_executed'}))
    out.append(make('Q08-H1','Configuration precedence across unrelated documentation',[
        'Fix resolve_limit(cli:Option<u32>,env:Option<u32>,config:Option<u32>)->u32 following docs/config-contract.md: first present CLI then environment then config, otherwise default 64. Present zero disables the limit and must not fall through.',
        'Other docs are archived distractors; confirm current contract source evidence. Preserve API and add assertions for all precedence combinations.'
    ],{'src/lib.rs':'''pub fn resolve_limit(cli:Option<u32>,env:Option<u32>,config:Option<u32>)->u32{config.or(env).or(cli).filter(|v|*v!=0).unwrap_or(32)}''',
       'docs/config-contract.md':'Current v4 config: CLI > environment > configuration > default 64. Zero explicitly disables limiting.',
       'docs/archive/v1.md':'Archived v1 only: default 32 and configuration file overrides environment.',
       'docs/archive/v2.md':'Archived v2 only: zero means use defaults.'},
       {'src/lib.rs':'''pub fn resolve_limit(cli:Option<u32>,env:Option<u32>,config:Option<u32>)->u32{cli.or(env).or(config).unwrap_or(64)}'''},
       '''use fixture::*;#[test]fn all_precedence(){for c in [None,Some(0),Some(7)]{for e in [None,Some(0),Some(8)]{for f in [None,Some(0),Some(9)]{assert_eq!(resolve_limit(c,e,f),c.or(e).or(f).unwrap_or(64));}}}}''',protected=['docs/**'],evidence={'required_source':'docs/config-contract.md','status':'not_executed'}))
    out.append(make('Q09-D1','Stale memory after API v2',[
        'Implement expiry_seconds(start:u64,ttl_seconds:u64)->Option<u64> for API v2: ttl is already seconds and checked addition yields expiry. Old v1 minutes rule is stale and must not influence this run. Current docs/api-v2.md is authoritative.',
        'Add assertions for zero, normal values and overflow. Apply current source-linked memory and exclude stale v1 memory.'
    ],{'src/lib.rs':'''pub fn expiry_seconds(start:u64,ttl_seconds:u64)->Option<u64>{start.checked_add(ttl_seconds.checked_mul(60)?)}''',
       'docs/api-v2.md':'Current API v2: ttl_seconds is seconds, expiry = checked start + ttl_seconds. v1 used minutes and is obsolete.'},
       {'src/lib.rs':'''pub fn expiry_seconds(start:u64,ttl_seconds:u64)->Option<u64>{start.checked_add(ttl_seconds)}'''},
       '''use fixture::*;#[test]fn api_v2(){assert_eq!(expiry_seconds(100,3),Some(103));assert_eq!(expiry_seconds(0,0),Some(0));assert_eq!(expiry_seconds(u64::MAX,0),Some(u64::MAX));assert_eq!(expiry_seconds(u64::MAX,1),None);}''',protected=['docs/**'],
       setup=hook('memory_source_transition',old_claim='API v1 TTL values are minutes; multiply TTL by sixty.',current_claim='API v2 TTL values are seconds; use checked addition only.',required_current_source='baseline_tree',stale_source='previous_api_v1_tree',required_events=['memory.applied','memory.excluded'])) )
    out.append(make('Q09-D2','Quarantined pinned skill rejects launch',[
        'A task pins bench-v2@1.0.0 but that immutable skill is quarantined. The harness must reject this task before any model/action dispatch. No implementation work is authorized after failed dependency resolution.',
        'Functional patch if run with an installed valid skill would make retry_allowed(attempt:u32,max:u32)->bool true only when attempt<max. This family scenario is PASS only for the expected pre-dispatch BLOCKED, not VERIFIED.'
    ],{'src/lib.rs':'''pub fn retry_allowed(attempt:u32,max:u32)->bool{attempt<=max}'''},
       {'src/lib.rs':'''pub fn retry_allowed(attempt:u32,max:u32)->bool{attempt<max}'''},
       '''use fixture::*;#[test]fn retry_boundary(){assert!(retry_allowed(0,1));assert!(!retry_allowed(1,1));assert!(!retry_allowed(0,0));assert!(!retry_allowed(u32::MAX,u32::MAX));}''',
       setup=hook('quarantined_skill',skill_key='bench-v2@1.0.0',expected_run_state='BLOCKED',expected_model_calls=0,expected_action_intents=0),skills=['bench-v2@1.0.0']))
    out.append(make('Q09-H1','Quarantined transitive skill dependency',[
        'Task pins bench-wrapper@1.0.0 whose exact-version dependency bench-encoding@1.0.0 is quarantined. Dependency graph resolution must reject before any model/action dispatch; an installed wrapper cannot override dependency quarantine.',
        'No implementation after resolution failure. The independent underlying task would implement ascii_lower(&str)->String lowering only ASCII uppercase and preserving all non-ASCII scalars. Expected scenario state is pre-dispatch BLOCKED.'
    ],{'src/lib.rs':'''pub fn ascii_lower(s:&str)->String{s.to_lowercase()}'''},
       {'src/lib.rs':'''pub fn ascii_lower(s:&str)->String{s.to_ascii_lowercase()}'''},
       '''use fixture::*;#[test]fn ascii_only(){assert_eq!(ascii_lower("AbC ÄΣ雪"),"abc ÄΣ雪");assert_eq!(ascii_lower(""),"");}''',
       setup=hook('quarantined_dependency',skill_key='bench-wrapper@1.0.0',dependency_key='bench-encoding@1.0.0',expected_run_state='BLOCKED',expected_model_calls=0,expected_action_intents=0),skills=['bench-wrapper@1.0.0']))
    out.append(make('Q10-D1','Portable cache backend under hard constraints',[
        'Implement portable_cache::Cache with new(), set(&str,i32), get(&str)->Option<i32> using an in-process std::collections::BTreeMap with no native service, network or OS-specific APIs. Updates replace prior values; empty/unicode keys work.',
        'Choose the portable backend from independently measured alternatives with hard requirements offline and Windows/macOS/Linux portability; reject unavailable remote backend regardless of latency. Preserve public API and add assertions. Strategy selection evidence is required separately from correct code.'
    ],{'src/lib.rs':'''pub mod portable_cache{#[derive(Default)]pub struct Cache;impl Cache{pub fn new()->Self{Self}pub fn set(&mut self,_key:&str,_value:i32){}pub fn get(&self,_key:&str)->Option<i32>{None}}}'''},
       {'src/lib.rs':'''pub mod portable_cache{#[derive(Default)]pub struct Cache{data:std::collections::BTreeMap<String,i32>}impl Cache{pub fn new()->Self{Self::default()}pub fn set(&mut self,key:&str,value:i32){self.data.insert(key.to_string(),value);}pub fn get(&self,key:&str)->Option<i32>{self.data.get(key).copied()}}}'''},
       '''use fixture::*;#[test]fn cache_contract(){let mut c=portable_cache::Cache::new();assert_eq!(c.get("x"),None);for(k,v)in[("",0),("雪",-7),("a",9)]{c.set(k,v);assert_eq!(c.get(k),Some(v));}c.set("a",3);assert_eq!(c.get("a"),Some(3));}''',
       setup=hook('strategy_selection',policy={'hard_requirements':['offline','portable']},alternatives=['portable-btree','remote-service'],expected_selected='portable-btree',required_measurements=['latency_ns'],measurements_status='not_measured')))
    out.append(make('Q10-D2','Select index for repeated immutable lookups',[
        'Implement Lookup::new(&[(u32,u32)]) and get(u32)->Option<u32> storing a std::collections::BTreeMap built once, first duplicate wins. Workload is 4096 immutable rows and 20000 repeated lookups. No network/native services; candidate get must use the index.',
        'Measure scan and indexed alternatives on the same fixture/environment with raw samples and hard-correctness checks, then select only from comparable evidence. Preserve API and add assertions; code alone does not prove strategy-family acceptance.'
    ],{'src/lib.rs':'''pub struct Lookup{rows:Vec<(u32,u32)>}impl Lookup{pub fn new(rows:&[(u32,u32)])->Self{Self{rows:rows.to_vec()}}pub fn get(&self,k:u32)->Option<u32>{self.rows.iter().find(|(x,_)|*x==k).map(|(_,v)|*v)}}'''},
       {'src/lib.rs':'''pub struct Lookup{index:std::collections::BTreeMap<u32,u32>}impl Lookup{pub fn new(rows:&[(u32,u32)])->Self{let mut index=std::collections::BTreeMap::new();for(k,v)in rows{index.entry(*k).or_insert(*v);}Self{index}}pub fn get(&self,k:u32)->Option<u32>{self.index.get(&k).copied()}}'''},
       '''use fixture::*;#[test]fn lookup_contract(){let c=Lookup::new(&[(1,2),(1,9),(3,4)]);assert_eq!(c.get(1),Some(2));assert_eq!(c.get(3),Some(4));assert_eq!(c.get(2),None);assert_eq!(Lookup::new(&[]).get(0),None);}''',structure={'kind':'lookup_index','status':'required'},
       setup=hook('strategy_selection',policy={'hard_requirements':['correct','offline']},alternatives=['linear-scan','btree-index'],required_measurements=['batch_latency_ns'],workload={'rows':4096,'lookups':20000},expected_selected='btree-index',measurements_status='not_measured')))
    out.append(make('Q10-H1','Bounded queue strategy respects memory hard limit',[
        'Implement Queue::new(capacity:usize), push(u32)->bool and pop()->Option<u32> with std::collections::VecDeque. Fixed capacity: full push rejects without mutation, capacity zero rejects all, FIFO preserved. No unlimited queue or external service.',
        'Select the bounded in-process strategy using measured alternatives and hard constraints fixed capacity and offline operation. Faster unbounded mechanism fails the hard capacity constraint. Preserve API and add assertions; strategy evidence is independently required.'
    ],{'src/lib.rs':'''pub struct Queue{data:std::collections::VecDeque<u32>}impl Queue{pub fn new(_capacity:usize)->Self{Self{data:Default::default()}}pub fn push(&mut self,v:u32)->bool{self.data.push_back(v);true}pub fn pop(&mut self)->Option<u32>{self.data.pop_front()}}'''},
       {'src/lib.rs':'''pub struct Queue{data:std::collections::VecDeque<u32>,capacity:usize}impl Queue{pub fn new(capacity:usize)->Self{Self{data:Default::default(),capacity}}pub fn push(&mut self,v:u32)->bool{if self.data.len()>=self.capacity{false}else{self.data.push_back(v);true}}pub fn pop(&mut self)->Option<u32>{self.data.pop_front()}}'''},
       '''use fixture::*;#[test]fn bounded_fifo(){let mut q=Queue::new(2);assert!(q.push(1));assert!(q.push(2));assert!(!q.push(3));assert_eq!(q.pop(),Some(1));assert!(q.push(4));assert_eq!(q.pop(),Some(2));assert_eq!(q.pop(),Some(4));assert_eq!(q.pop(),None);let mut zero=Queue::new(0);assert!(!zero.push(1));assert_eq!(zero.pop(),None);}''',
       setup=hook('strategy_selection',policy={'hard_requirements':['bounded_capacity','offline']},alternatives=['bounded-vecdeque','unbounded-vecdeque'],expected_selected='bounded-vecdeque',required_measurements=['push_pop_latency_ns'],measurements_status='not_measured')))
    return out
