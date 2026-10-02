"""Frozen, dependency-free Rust benchmark definitions. Oracles stay outside seeds."""
from textwrap import dedent


def text(value):
    return dedent(value).strip()+'\n'


def make(task_id,title,requirements,baseline,control,oracle,**extra):
    return {'id':task_id,'family':task_id.split('-')[0],'title':title,
            'split':'holdout' if task_id.endswith('H1') else 'dev',
            'oracle_visibility':'withheld_from_agent_repository','hidden_from_developers':False,
            'requirements':requirements,
            'baseline':{k:text(v) for k,v in baseline.items()},
            'control':{k:text(v) for k,v in control.items()},'oracle':text(oracle),**extra}


def definitions():
    out=[]
    out.append(make('Q01-D1','CLI substring filter and pagination',[
        'Implement filter_page(rows: &[&str], query: &str, offset: usize, limit: usize) -> Vec<String>: keep case-sensitive substring matches in input order, then skip offset and take at most limit. Empty query matches every row; zero limit or offset past end gives an empty vector; usize::MAX never overflows.',
        'CLI harness_benchmark_fixture accepts QUERY OFFSET LIMIT, filters the fixed rows alpha,beta,alphabet,gamma, and prints each result on its own line. Invalid integer arguments or wrong arity exit 2. Preserve the library and CLI contract; add regression assertions.'
    ],{'src/lib.rs':'''pub fn filter_page(rows: &[&str], _query: &str, _offset: usize, limit: usize) -> Vec<String> { rows.iter().take(limit).map(|s| s.to_string()).collect() }''',
       'src/main.rs':'''fn main() { let a: Vec<String> = std::env::args().collect(); if a.len()!=4 {std::process::exit(2)}; let (Ok(offset),Ok(limit))=(a[2].parse::<usize>(),a[3].parse::<usize>()) else {std::process::exit(2)}; for row in harness_benchmark_fixture::filter_page(&["alpha","beta","alphabet","gamma"],&a[1],offset,limit) { println!("{row}") } }'''},
       {'src/lib.rs':'''pub fn filter_page(rows: &[&str], query: &str, offset: usize, limit: usize) -> Vec<String> { rows.iter().filter(|s| s.contains(query)).skip(offset).take(limit).map(|s| s.to_string()).collect() }
       #[cfg(test)] mod tests { use super::*; #[test] fn boundaries(){assert_eq!(filter_page(&["a","b","ab"],"a",1,10),vec!["ab"]);assert!(filter_page(&["a"],"",usize::MAX,usize::MAX).is_empty());}}'''},
       '''use fixture::*;
       #[test] fn filter_then_page(){ assert_eq!(filter_page(&["alpha","beta","alphabet","gamma"],"alp",1,7),vec!["alphabet"]);assert_eq!(filter_page(&["a","b"],"",0,usize::MAX),vec!["a","b"]);assert!(filter_page(&["a"],"a",usize::MAX,1).is_empty());assert!(filter_page(&["a"],"a",0,0).is_empty()); }
       #[test] fn cli_contract(){let binary=std::env::var("FIXTURE_BINARY").unwrap();let run=std::process::Command::new(&binary).args(["alp","1","5"]).output().unwrap();assert!(run.status.success());assert_eq!(String::from_utf8(run.stdout).unwrap(),"alphabet\\n");assert_eq!(std::process::Command::new(binary).args(["x","invalid","1"]).status().unwrap().code(),Some(2));}''',has_binary=True))
    out.append(make('Q01-D2','Compatible JSON output escaping',[
        'Implement record_json(id: u32, name: &str, active: bool) -> String producing exactly {"id":N,"name":"ESCAPED","active":BOOL}, no whitespace. Escape JSON quotes, backslashes, newline, carriage return, tab and all remaining U+0000..U+001F as lowercase \\u00xx; preserve other Unicode.',
        'CLI takes ID NAME ACTIVE (true/false only), emits one JSON line; wrong arity/invalid ID/invalid bool exit 2. Existing id/active field names and order are stable. Add assertions for escaping and Unicode.'
    ],{'src/lib.rs':'''pub fn record_json(id:u32,name:&str,active:bool)->String{format!("{{\\\"id\\\":{id},\\\"name\\\":\\\"{name}\\\",\\\"active\\\":{active}}}")}''',
       'src/main.rs':'''fn main(){let a:Vec<String>=std::env::args().collect();if a.len()!=4{std::process::exit(2)};let(Ok(id),Ok(active))=(a[1].parse::<u32>(),a[3].parse::<bool>())else{std::process::exit(2)};println!("{}",harness_benchmark_fixture::record_json(id,&a[2],active));}'''},
       {'src/lib.rs':r'''pub fn record_json(id:u32,name:&str,active:bool)->String{let mut e=String::new();for c in name.chars(){match c{ '"'=>e.push_str("\\\""),'\\'=>e.push_str("\\\\"),'\n'=>e.push_str("\\n"),'\r'=>e.push_str("\\r"),'\t'=>e.push_str("\\t"),c if (c as u32)<32=>e.push_str(&format!("\\u{:04x}",c as u32)),c=>e.push(c)}}format!("{{\"id\":{id},\"name\":\"{e}\",\"active\":{active}}}")}'''},
       r'''use fixture::*;
       #[test] fn exact_json(){assert_eq!(record_json(7,"雪",true),"{\"id\":7,\"name\":\"雪\",\"active\":true}");assert_eq!(record_json(0,"a\"b\\c\n\r\t\u{1}",false),"{\"id\":0,\"name\":\"a\\\"b\\\\c\\n\\r\\t\\u0001\",\"active\":false}");}
       #[test] fn cli_contract(){let run=std::process::Command::new(std::env::var("FIXTURE_BINARY").unwrap()).args(["3","x","false"]).output().unwrap();assert!(run.status.success());assert_eq!(String::from_utf8(run.stdout).unwrap(),"{\"id\":3,\"name\":\"x\",\"active\":false}\n");}''',has_binary=True))
    out.append(make('Q01-H1','Stable duplicate-aware merge API',[
        'Implement merge_unique(left: &[i32], right: &[i32]) -> Vec<i32> retaining the first occurrence of every value across left followed by right, in encounter order. Do not sort; handle empty inputs, repeated negatives, zero and i32 bounds.',
        'Add assertion-based tests without changing the public signature or Cargo files.'
    ],{'src/lib.rs':'''pub fn merge_unique(left:&[i32],right:&[i32])->Vec<i32>{left.iter().chain(right).copied().collect()}'''},
       {'src/lib.rs':'''pub fn merge_unique(left:&[i32],right:&[i32])->Vec<i32>{let mut seen=std::collections::BTreeSet::new();left.iter().chain(right).copied().filter(|v|seen.insert(*v)).collect()}
       #[cfg(test)]mod tests{use super::*;#[test]fn order(){assert_eq!(merge_unique(&[3,1,3],&[1,2]),vec![3,1,2]);}}'''},
       '''use fixture::*;#[test]fn stable_union(){assert_eq!(merge_unique(&[3,1,3,-1],&[-1,2,0,3]),vec![3,1,-1,2,0]);assert_eq!(merge_unique(&[],&[]),Vec::<i32>::new());assert_eq!(merge_unique(&[i32::MAX,i32::MIN],&[i32::MAX]),vec![i32::MAX,i32::MIN]);}'''))
    out.append(make('Q02-D1','Clamp mixed-width bounds',[
        'Fix clamp_i64(value: i64, low: i64, high: i64) -> i64 for low<=high: return inclusive clamp without overflow or narrowing. Handle equal bounds, negative values and i64 extremes.',
        'Add regression assertions reproducing the faulty below/inside/above behavior.'
    ],{'src/lib.rs':'''pub fn clamp_i64(value:i64,low:i64,high:i64)->i64{if value<low{high}else if value>high{low}else{value}}'''},
       {'src/lib.rs':'''pub fn clamp_i64(value:i64,low:i64,high:i64)->i64{value.max(low).min(high)}
       #[cfg(test)]mod tests{use super::*;#[test]fn regression(){assert_eq!(clamp_i64(-10,0,5),0);assert_eq!(clamp_i64(9,0,5),5);assert_eq!(clamp_i64(i64::MIN,i64::MIN,i64::MAX),i64::MIN);}}'''},
       '''use fixture::*;#[test]fn exhaustive_small(){for low in -4..=4{for high in low..=4{for value in -8..=8{assert_eq!(clamp_i64(value,low,high),if value<low{low}else if value>high{high}else{value});}}}}#[test]fn extremes(){assert_eq!(clamp_i64(i64::MIN,-1,i64::MAX),-1);assert_eq!(clamp_i64(i64::MAX,i64::MIN,5),5);assert_eq!(clamp_i64(0,i64::MIN,i64::MAX),0);assert_eq!(clamp_i64(5,-7,-7),-7);}'''))
    out.append(make('Q02-D2','Unicode character offsets',[
        'Fix char_slice(input: &str, start: usize, count: usize) -> String where start/count count Unicode scalar values, never UTF-8 bytes. Return available suffix if count exceeds length, empty for start beyond end/count zero; usize::MAX must not overflow or panic. Combining marks remain separate scalar values.',
        'Add assertions covering ASCII, multibyte text, emoji and combining marks.'
    ],{'src/lib.rs':'''pub fn char_slice(input:&str,start:usize,count:usize)->String{input.get(start..start.saturating_add(count).min(input.len())).unwrap_or("").to_string()}'''},
       {'src/lib.rs':'''pub fn char_slice(input:&str,start:usize,count:usize)->String{input.chars().skip(start).take(count).collect()}
       #[cfg(test)]mod tests{use super::*;#[test]fn unicode(){assert_eq!(char_slice("a雪🙂b",1,2),"雪🙂");assert_eq!(char_slice("abc",usize::MAX,usize::MAX),"");}}'''},
       '''use fixture::*;#[test]fn scalar_offsets(){for (s,a,n,want) in [("a雪🙂b",1,2,"雪🙂"),("e\\u{301}x",1,1,"\\u{301}"),("привет",2,2,"ив"),("",0,9,""),("ab",1,usize::MAX,"b"),("abc",usize::MAX,2,""),("abc",0,0,"")]{assert_eq!(char_slice(s,a,n),want);}}'''))
    out.append(make('Q02-H1','Overflow-safe ceiling division',[
        'Fix ceil_div(value: u64, divisor: u64) -> Option<u64>. divisor=0 returns None; otherwise integer ceiling, including value=0 and u64::MAX without overflow. Preserve the API.',
        'Add regression assertions for exact multiples, nonmultiples, zero and extremes.'
    ],{'src/lib.rs':'''pub fn ceil_div(value:u64,divisor:u64)->Option<u64>{if divisor==0{None}else{Some(value/divisor)}}'''},
       {'src/lib.rs':'''pub fn ceil_div(value:u64,divisor:u64)->Option<u64>{if divisor==0{None}else{Some(value/divisor+u64::from(value%divisor!=0))}}
       #[cfg(test)]mod tests{use super::*;#[test]fn ceiling(){assert_eq!(ceil_div(5,2),Some(3));assert_eq!(ceil_div(u64::MAX,2),Some(1<<63));}}'''},
       '''use fixture::*;#[test]fn corpus(){for value in 0..=100{for d in 1..=17{assert_eq!(ceil_div(value,d),Some(((value as u128+d as u128-1)/d as u128)as u64));}}assert_eq!(ceil_div(2,0),None);assert_eq!(ceil_div(u64::MAX,1),Some(u64::MAX));assert_eq!(ceil_div(u64::MAX,2),Some(1<<63));}'''))
    out.append(make('Q03-D1','Build an index preserving first duplicate',[
        'Refactor Catalog::new(rows: &[(String,i32)]) and lookup(&self,key:&str)->Option<i32> to build/store a std::collections::BTreeMap<String,i32> once in new and perform lookup through its get method. Duplicate keys keep the first value. lookup must not scan entries or rebuild the index.',
        'Preserve behavior including unknown keys/empty rows/Unicode; add assertions. No speedup claim without measurements.'
    ],{'src/lib.rs':'''pub struct Catalog{entries:Vec<(String,i32)>}impl Catalog{pub fn new(rows:&[(String,i32)])->Self{Self{entries:rows.to_vec()}}pub fn lookup(&self,key:&str)->Option<i32>{self.entries.iter().find(|(k,_)|k==key).map(|(_,v)|*v)}}'''},
       {'src/lib.rs':'''pub struct Catalog{index:std::collections::BTreeMap<String,i32>}impl Catalog{pub fn new(rows:&[(String,i32)])->Self{let mut index=std::collections::BTreeMap::new();for(k,v)in rows{index.entry(k.clone()).or_insert(*v);}Self{index}}pub fn lookup(&self,key:&str)->Option<i32>{self.index.get(key).copied()}}
       #[cfg(test)]mod tests{use super::*;#[test]fn first(){let c=Catalog::new(&[("a".into(),3),("a".into(),4)]);assert_eq!(c.lookup("a"),Some(3));}}'''},
       '''use fixture::*;#[test]fn equivalent_corpus(){let rows=vec![("雪".to_string(),9),("a".to_string(),2),("雪".to_string(),-1)];let c=Catalog::new(&rows);for key in ["雪","a","","missing"]{assert_eq!(c.lookup(key),rows.iter().find(|(k,_)|k==key).map(|(_,v)|*v));}assert_eq!(Catalog::new(&[]).lookup("a"),None);}''',structure={'kind':'catalog_index','status':'required'}))
    out.append(make('Q03-D2','Extract reusable JSON string serialization',[
        'Behavior-preserving refactor: public name_json(&str)->String and label_json(&str)->String both produce a JSON string literal escaping quotes, backslash and newline only. Extract that escaping into exactly one private fn encode_json(&str)->String called by both public functions. The helper returns the complete quoted string.',
        'No behavior changes on the independent corpus; add assertions. Do not leave duplicate escaping loops in the public functions.'
    ],{'src/lib.rs':r'''pub fn name_json(s:&str)->String{format!("\"{}\"",s.replace('\\',"\\\\").replace('"',"\\\"").replace('\n',"\\n"))}pub fn label_json(s:&str)->String{format!("\"{}\"",s.replace('\\',"\\\\").replace('"',"\\\"").replace('\n',"\\n"))}'''},
       {'src/lib.rs':r'''fn encode_json(s:&str)->String{format!("\"{}\"",s.replace('\\',"\\\\").replace('"',"\\\"").replace('\n',"\\n"))}pub fn name_json(s:&str)->String{encode_json(s)}pub fn label_json(s:&str)->String{encode_json(s)}'''},
       r'''use fixture::*;#[test]fn preserved(){for(s,want)in[("","\"\""),("雪","\"雪\""),("a\"b","\"a\\\"b\""),("x\\y\nz","\"x\\\\y\\nz\"")]{assert_eq!(name_json(s),want);assert_eq!(label_json(s),want);}}''',structure={'kind':'json_helper','status':'required'}))
    out.append(make('Q03-H1','Extract CSV quoting preserving exact bytes',[
        'Refactor csv_cell(&str)->String and csv_header(&str)->String to call exactly one private fn encode_csv(&str)->String. Quote cells containing comma, quote, CR or LF and double internal quotes; other cells are unchanged. Empty string remains empty.',
        'Preserve the exact CSV bytes on the independent corpus; eliminate duplicate quoting logic from both public functions.'
    ],{'src/lib.rs':r'''pub fn csv_cell(s:&str)->String{if s.contains([',','"','\r','\n']){format!("\"{}\"",s.replace('"',"\"\""))}else{s.to_string()}}pub fn csv_header(s:&str)->String{if s.contains([',','"','\r','\n']){format!("\"{}\"",s.replace('"',"\"\""))}else{s.to_string()}}'''},
       {'src/lib.rs':r'''fn encode_csv(s:&str)->String{if s.contains([',','"','\r','\n']){format!("\"{}\"",s.replace('"',"\"\""))}else{s.to_string()}}pub fn csv_cell(s:&str)->String{encode_csv(s)}pub fn csv_header(s:&str)->String{encode_csv(s)}'''},
       r'''use fixture::*;#[test]fn preserved(){for(s,want)in[("",""),("abc","abc"),("a,b","\"a,b\""),("a\"b","\"a\"\"b\""),("a\nb","\"a\nb\""),("雪","雪")]{assert_eq!(csv_cell(s),want);assert_eq!(csv_header(s),want);}}''',structure={'kind':'csv_helper','status':'required'}))
    out.append(make('Q04-D1','Priority across library/CLI/JSON components',[
        'Implement public Priority {Low,Normal,High}, parse_priority(&str)->Option<Priority>, and job_json(id:u32,priority:Priority)->String. Accepted lowercase values low/normal/high only; stable JSON exactly {"id":N,"priority":"VALUE"}.',
        'CLI ID PRIORITY validates both and prints JSON newline; invalid priority/ID/arity exits 2. Update parse and render components together and add assertions.'
    ],{'src/lib.rs':'''pub mod parse;pub mod render;#[derive(Debug,Clone,Copy,PartialEq,Eq)]pub enum Priority{Low,Normal,High}pub use parse::parse_priority;pub use render::job_json;''',
       'src/parse.rs':'''use crate::Priority;pub fn parse_priority(s:&str)->Option<Priority>{match s{"normal"=>Some(Priority::Normal),_=>None}}''',
       'src/render.rs':'''use crate::Priority;pub fn job_json(id:u32,_p:Priority)->String{format!("{{\\\"id\\\":{id}}}")}''',
       'src/main.rs':'''fn main(){let a:Vec<String>=std::env::args().collect();if a.len()!=3{std::process::exit(2)};let Some(p)=harness_benchmark_fixture::parse_priority(&a[2])else{std::process::exit(2)};let Ok(id)=a[1].parse()else{std::process::exit(2)};println!("{}",harness_benchmark_fixture::job_json(id,p));}'''},
       {'src/parse.rs':'''use crate::Priority;pub fn parse_priority(s:&str)->Option<Priority>{match s{"low"=>Some(Priority::Low),"normal"=>Some(Priority::Normal),"high"=>Some(Priority::High),_=>None}}''',
        'src/render.rs':'''use crate::Priority;pub fn job_json(id:u32,p:Priority)->String{let p=match p{Priority::Low=>"low",Priority::Normal=>"normal",Priority::High=>"high"};format!("{{\\\"id\\\":{id},\\\"priority\\\":\\\"{p}\\\"}}") }'''},
       '''use fixture::*;#[test]fn cross_components(){for(s,p)in[("low",Priority::Low),("normal",Priority::Normal),("high",Priority::High)]{assert_eq!(parse_priority(s),Some(p));assert_eq!(job_json(8,p),format!("{{\\\"id\\\":8,\\\"priority\\\":\\\"{s}\\\"}}"));}assert_eq!(parse_priority("High"),None);}#[test]fn cli(){let run=std::process::Command::new(std::env::var("FIXTURE_BINARY").unwrap()).args(["3","high"]).output().unwrap();assert!(run.status.success());assert_eq!(String::from_utf8(run.stdout).unwrap(),"{\\\"id\\\":3,\\\"priority\\\":\\\"high\\\"}\\n");}''',has_binary=True,nodes=[('parse',['src/parse.rs']),('render',['src/render.rs'])]))
    out.append(make('Q04-D2','Cancellation crosses worker and API',[
        'Fix CancellationToken::cancel and is_cancelled using shared Arc<AtomicBool>, so cloned tokens observe cancellation. worker::run_steps(&CancellationToken,total:usize)->usize checks cancellation before every unit of work and returns zero for an already cancelled token.',
        'api::cancel_then_run(total) creates a token, cancels a clone and returns worker result. Preserve all public signatures; add cross-component assertions.'
    ],{'src/lib.rs':'''pub mod token;pub mod worker;pub mod api;pub use token::CancellationToken;''',
       'src/token.rs':'''#[derive(Clone,Default)]pub struct CancellationToken{cancelled:bool}impl CancellationToken{pub fn cancel(&self){}pub fn is_cancelled(&self)->bool{self.cancelled}}''',
       'src/worker.rs':'''use crate::CancellationToken;pub fn run_steps(_token:&CancellationToken,total:usize)->usize{total}''',
       'src/api.rs':'''pub fn cancel_then_run(total:usize)->usize{let token=crate::CancellationToken::default();token.clone().cancel();crate::worker::run_steps(&token,total)}'''},
       {'src/token.rs':'''#[derive(Clone,Default)]pub struct CancellationToken{cancelled:std::sync::Arc<std::sync::atomic::AtomicBool>}impl CancellationToken{pub fn cancel(&self){self.cancelled.store(true,std::sync::atomic::Ordering::SeqCst)}pub fn is_cancelled(&self)->bool{self.cancelled.load(std::sync::atomic::Ordering::SeqCst)}}''',
        'src/worker.rs':'''use crate::CancellationToken;pub fn run_steps(token:&CancellationToken,total:usize)->usize{let mut done=0;while done<total{if token.is_cancelled(){break}done+=1;}done}'''},
       '''use fixture::*;#[test]fn shared_cancellation(){let t=CancellationToken::default();assert_eq!(worker::run_steps(&t,7),7);let child=t.clone();child.cancel();assert!(t.is_cancelled());assert_eq!(worker::run_steps(&t,7),0);assert_eq!(api::cancel_then_run(usize::MAX),0);assert_eq!(worker::run_steps(&CancellationToken::default(),0),0);}''',nodes=[('token',['src/token.rs']),('worker',['src/worker.rs'])]))
    out.append(make('Q04-H1','Checked minor-unit currency conversion',[
        'Fix parse::parse_cents(&str)->Option<u64> for ASCII nonnegative money: one or more digits optionally followed by a dot and exactly two digits. No sign/whitespace/exponent. Reject overflow. formatter::format_cents(u64)->String prints dollars plus two digits.',
        'Public normalize(&str)->Option<String> composes parsing and formatting. Preserve signatures and add end-to-end roundtrip assertions including u64::MAX.'
    ],{'src/lib.rs':'''pub mod parse;pub mod formatter;pub fn normalize(s:&str)->Option<String>{parse::parse_cents(s).map(formatter::format_cents)}''',
       'src/parse.rs':'''pub fn parse_cents(s:&str)->Option<u64>{s.parse::<u64>().ok()}''',
       'src/formatter.rs':'''pub fn format_cents(n:u64)->String{n.to_string()}'''},
       {'src/parse.rs':'''pub fn parse_cents(s:&str)->Option<u64>{let mut p=s.split('.');let whole=p.next()?;let frac=p.next();if p.next().is_some()||whole.is_empty()||!whole.bytes().all(|b|b.is_ascii_digit()){return None}let f=match frac{None=>0,Some(f)if f.len()==2&&f.bytes().all(|b|b.is_ascii_digit())=>f.parse().ok()?,_=>return None};whole.parse::<u64>().ok()?.checked_mul(100)?.checked_add(f)}''',
        'src/formatter.rs':'''pub fn format_cents(n:u64)->String{format!("{}.{:02}",n/100,n%100)}'''},
       '''use fixture::*;#[test]fn roundtrips(){for n in [0,1,99,100,12345,u64::MAX]{let rendered=formatter::format_cents(n);assert_eq!(parse::parse_cents(&rendered),Some(n));assert_eq!(normalize(&rendered),Some(rendered));}assert_eq!(normalize("12"),Some("12.00".into()));for bad in ["","-1"," 1","1.2","1.234","1e2","18446744073709551616"]{assert_eq!(normalize(bad),None);}}''',nodes=[('parse',['src/parse.rs']),('format',['src/formatter.rs'])]))
    out.append(make('Q05-D1','Half-open interval agreement after integration',[
        'Fix scheduling to use half-open [start,end) intervals in both overlap and billing components. overlap::overlaps(a_start,a_end,b_start,b_end) is false for touching or empty intervals. bill::billable_minutes(start,end) returns end-start for start<=end and zero for reversed bounds.',
        'public schedule_charge returns billable minutes only when request interval overlaps the available interval; preserve signatures and add integrated boundary assertions.'
    ],{'src/lib.rs':'''pub mod overlap;pub mod bill;pub fn schedule_charge(s:u32,e:u32,a:u32,b:u32)->u32{if overlap::overlaps(s,e,a,b){bill::billable_minutes(s,e)}else{0}}''',
       'src/overlap.rs':'''pub fn overlaps(a:u32,b:u32,c:u32,d:u32)->bool{a<=d&&c<=b}''',
       'src/bill.rs':'''pub fn billable_minutes(s:u32,e:u32)->u32{e.saturating_sub(s)+1}'''},
       {'src/overlap.rs':'''pub fn overlaps(a:u32,b:u32,c:u32,d:u32)->bool{a<b&&c<d&&a<d&&c<b}''',
        'src/bill.rs':'''pub fn billable_minutes(s:u32,e:u32)->u32{e.saturating_sub(s)}'''},
       '''use fixture::*;#[test]fn integrated_boundaries(){assert_eq!(schedule_charge(10,20,20,30),0);assert_eq!(schedule_charge(10,20,19,30),10);assert_eq!(schedule_charge(10,10,0,20),0);assert_eq!(schedule_charge(20,10,0,30),0);assert_eq!(bill::billable_minutes(0,u32::MAX),u32::MAX);for a in 0..5{for b in 0..5{for c in 0..5{for d in 0..5{assert_eq!(overlap::overlaps(a,b,c,d),a<b&&c<d&&a.max(c)<b.min(d));}}}}}''',nodes=[('overlap',['src/overlap.rs']),('bill',['src/bill.rs'])]))
    out.append(make('Q05-D2','Optional timeout field compatibility',[
        'Fix parser::parse_timeout(&str)->Option<Option<u32>>: empty string or "none" means valid absent timeout; ASCII nonnegative integer means present including zero; malformed/overflow is invalid None.',
        'consumer::effective_timeout(Option<u32>,default:u32) uses default only for None, preserves Some(0). Public configured_timeout combines parsing and consumer; add integrated compatibility assertions.'
    ],{'src/lib.rs':'''pub mod parser;pub mod consumer;pub fn configured_timeout(raw:&str,default:u32)->Option<u32>{parser::parse_timeout(raw).map(|x|consumer::effective_timeout(x,default))}''',
       'src/parser.rs':'''pub fn parse_timeout(s:&str)->Option<Option<u32>>{s.parse().ok().map(Some)}''',
       'src/consumer.rs':'''pub fn effective_timeout(v:Option<u32>,default:u32)->u32{v.filter(|x|*x!=0).unwrap_or(default)}'''},
       {'src/parser.rs':'''pub fn parse_timeout(s:&str)->Option<Option<u32>>{if s.is_empty()||s=="none"{Some(None)}else if s.bytes().all(|b|b.is_ascii_digit()){s.parse().ok().map(Some)}else{None}}''',
        'src/consumer.rs':'''pub fn effective_timeout(v:Option<u32>,default:u32)->u32{v.unwrap_or(default)}'''},
       '''use fixture::*;#[test]fn integrated_option(){assert_eq!(configured_timeout("",9),Some(9));assert_eq!(configured_timeout("none",9),Some(9));assert_eq!(configured_timeout("0",9),Some(0));assert_eq!(configured_timeout("4294967295",9),Some(u32::MAX));for bad in ["-1"," 0","null","4294967296"]{assert_eq!(configured_timeout(bad,9),None);}}''',nodes=[('parser',['src/parser.rs']),('consumer',['src/consumer.rs'])]))
    out.append(make('Q05-H1','Exclusive cursor pagination handoff',[
        'Fix store::after_cursor(ids:&[u32],cursor:Option<u32>)->Vec<u32> to include every input ID strictly greater than a present cursor, or all IDs if absent, preserving order. page::take_page takes at most limit entries and returns (entries,last_returned_id), cursor None for empty page.',
        'public next_page composes both. Cursor value zero is valid; no duplicate boundary item between pages; preserve API and add integrated assertions.'
    ],{'src/lib.rs':'''pub mod store;pub mod page;pub fn next_page(ids:&[u32],cursor:Option<u32>,limit:usize)->(Vec<u32>,Option<u32>){page::take_page(store::after_cursor(ids,cursor),limit)}''',
       'src/store.rs':'''pub fn after_cursor(ids:&[u32],cursor:Option<u32>)->Vec<u32>{ids.iter().copied().filter(|id|cursor.is_none_or(|c|*id>=c)).collect()}''',
       'src/page.rs':'''pub fn take_page(ids:Vec<u32>,limit:usize)->(Vec<u32>,Option<u32>){let cursor=ids.last().copied();(ids.into_iter().take(limit).collect(),cursor)}'''},
       {'src/store.rs':'''pub fn after_cursor(ids:&[u32],cursor:Option<u32>)->Vec<u32>{ids.iter().copied().filter(|id|cursor.is_none_or(|c|*id>c)).collect()}''',
        'src/page.rs':'''pub fn take_page(ids:Vec<u32>,limit:usize)->(Vec<u32>,Option<u32>){let rows:Vec<_>=ids.into_iter().take(limit).collect();let cursor=rows.last().copied();(rows,cursor)}'''},
       '''use fixture::*;#[test]fn handoff(){let(p,c)=next_page(&[0,1,2,3,4],None,2);assert_eq!(p,vec![0,1]);assert_eq!(c,Some(1));assert_eq!(next_page(&[0,1,2,3,4],c,2),(vec![2,3],Some(3)));assert_eq!(next_page(&[0,1],Some(0),9),(vec![1],Some(1)));assert_eq!(next_page(&[1,2],None,0),(vec![],None));assert_eq!(next_page(&[],None,9),(vec![],None));}''',nodes=[('store',['src/store.rs']),('page',['src/page.rs'])]))
    return out
