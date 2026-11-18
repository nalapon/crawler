use super::sampling::Process;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Memory,
    Cpu,
    Disk,
}
impl Metric {
    pub const ALL: [Self; 3] = [Self::Memory, Self::Cpu, Self::Disk];
    pub fn title(self) -> &'static str {
        match self {
            Self::Memory => "Memory",
            Self::Cpu => "CPU",
            Self::Disk => "Disk",
        }
    }
    pub fn column(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Cpu => "cpu",
            Self::Disk => "disk",
        }
    }
    pub fn value(self, p: &Process) -> Option<f64> {
        match self {
            Self::Memory => p.memory,
            Self::Cpu => p.cpu,
            Self::Disk => p.read.zip(p.written).map(|(r, w)| r + w),
        }
    }
    pub fn format(self, value: Option<f64>) -> String {
        let Some(value) = value else {
            return "—".into();
        };
        if self == Self::Cpu {
            return format!("{value:.1}%");
        }
        use objc2_foundation::{NSByteCountFormatter, NSByteCountFormatterCountStyle};
        let count = value.clamp(0.0, (i64::MAX - 1024) as f64) as i64;
        let bytes = NSByteCountFormatter::stringFromByteCount_countStyle(
            count,
            NSByteCountFormatterCountStyle::Memory,
        )
        .to_string();
        if self == Self::Disk {
            format!("{bytes}/s")
        } else {
            bytes
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub processes: Vec<Arc<Process>>,
}
pub fn sum(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    values.flatten().reduce(|a, b| a + b)
}
fn application(p: &Process) -> Option<String> {
    p.executable
        .find(".app/")
        .map(|i| p.executable[..i + 4].to_string())
}
pub fn groups(processes: &[Arc<Process>], query: &str) -> Vec<Group> {
    let by_pid: HashMap<_, _> = processes.iter().map(|p| (p.id.pid, p)).collect();
    let root = |p: &Process| {
        let mut current = p;
        let mut seen = HashSet::new();
        while current.parent > 1 && seen.insert(current.id.pid) {
            let Some(parent) = by_pid.get(&current.parent).copied() else {
                break;
            };
            if application(parent).is_some() {
                break;
            }
            current = parent;
        }
        (current.id.key(), current.name.clone())
    };
    let mut grouped: HashMap<String, Group> = HashMap::new();
    for p in processes {
        let (id, name) = if let Some(app) = application(p) {
            let name = std::path::Path::new(&app)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.name.clone());
            (app, name)
        } else {
            root(p)
        };
        if !query.is_empty() && !matches(query, &format!("{name} {} {}", p.name, p.id.pid)) {
            continue;
        }
        grouped
            .entry(id.clone())
            .or_insert_with(|| Group {
                id,
                name,
                processes: Vec::new(),
            })
            .processes
            .push(p.clone());
    }
    grouped.into_values().collect()
}

/// The process filter's bounded bit-parallel fuzzy subsequence matcher.
fn normalized(text: &str) -> String {
    text.nfd()
        .filter(|&c| !is_combining_mark(c))
        .flat_map(char::to_lowercase)
        .collect()
}
pub fn matches(query: &str, text: &str) -> bool {
    let words: Vec<_> = query.split_whitespace().collect();
    if words.len() > 16 {
        return false;
    }
    let text = normalized(text);
    words.iter().all(|word| {
        let word = normalized(word);
        let bytes = word.as_bytes();
        if bytes.len() > 256 {
            return false;
        }
        if bytes.len() >= 64 {
            let mut at = 0;
            for byte in text.bytes() {
                if bytes.get(at) == Some(&byte) {
                    at += 1;
                }
            }
            return at == bytes.len();
        }
        let typos = if bytes.is_ascii() {
            (bytes.len() / 4).min(2)
        } else {
            0
        };
        let mut masks = [0_u64; 256];
        for (i, &b) in bytes.iter().enumerate() {
            masks[b as usize] |= 1 << i;
        }
        let goal = 1_u64 << bytes.len();
        let (mut zero, mut one, mut two) = (1_u64, 3_u64, 7_u64);
        for b in text.bytes() {
            zero |= (zero & masks[b as usize]) << 1;
            one |= ((one & masks[b as usize]) << 1) | (zero << 1);
            two |= ((two & masks[b as usize]) << 1) | (one << 1);
            if (match typos {
                0 => zero,
                1 => one,
                _ => two,
            }) & goal
                != 0
            {
                return true;
            }
        }
        bytes.is_empty()
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListRow {
    pub id: String,
    pub group: String,
    pub name: String,
    pub subtitle: String,
    pub processes: Vec<Arc<Process>>,
    pub child: bool,
}
impl ListRow {
    pub fn value(&self, m: Metric) -> Option<f64> {
        sum(self.processes.iter().map(|p| m.value(p)))
    }
}

#[derive(Debug)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub value: f64,
    // Every sampled process occurs at one leaf, including processes with no measured usage.
    pub process: Option<Arc<Process>>,
    children: Vec<Node>,
}

impl Node {
    pub fn group(id: String, name: String, children: Vec<Node>) -> Self {
        let value = children.iter().map(|n| n.value).sum();
        let children = limited(children, &id);
        Self {
            id,
            name,
            value,
            process: None,
            children,
        }
    }

    pub fn visible(&self) -> &[Node] {
        &self.children[..self.children.partition_point(|n| n.value > 0.0)]
    }

    pub fn members(&self, into: &mut Vec<Arc<Process>>) {
        if let Some(process) = &self.process {
            into.push(process.clone());
        }
        for node in &self.children {
            node.members(into);
        }
    }
}

pub fn forest(processes: &[Arc<Process>], metric: Metric) -> Vec<Node> {
    let mut by_parent: HashMap<i32, Vec<&Arc<Process>>> = HashMap::new();
    let ids: HashSet<_> = processes.iter().map(|p| p.id.pid).collect();
    for process in processes {
        if process.id.pid != process.parent {
            by_parent.entry(process.parent).or_default().push(process);
        }
    }
    fn node(
        p: &Arc<Process>,
        by_parent: &HashMap<i32, Vec<&Arc<Process>>>,
        metric: Metric,
        seen: &mut HashSet<i32>,
    ) -> Option<Node> {
        if !seen.insert(p.id.pid) {
            return None;
        }
        let mut children: Vec<_> = by_parent
            .get(&p.id.pid)
            .into_iter()
            .flatten()
            .filter_map(|child| node(child, by_parent, metric, seen))
            .collect();
        let id = p.id.key();
        let mut own = Node {
            id: id.clone(),
            name: p.name.clone(),
            value: metric.value(p).unwrap_or(0.0),
            process: Some(p.clone()),
            children: vec![],
        };
        if children.is_empty() {
            return Some(own);
        }
        own.id = format!("own:{id}");
        children.push(own);
        Some(Node::group(id, p.name.clone(), children))
    }
    let mut seen = HashSet::new();
    // Visit disconnected cycles too, once every ordinary root is accounted for.
    processes
        .iter()
        .filter(|p| !ids.contains(&p.parent) || p.parent == p.id.pid)
        .chain(processes)
        .filter_map(|p| node(p, &by_parent, metric, &mut seen))
        .collect()
}

fn order(a: &Node, b: &Node) -> std::cmp::Ordering {
    b.value
        .total_cmp(&a.value)
        .then_with(|| a.name.cmp(&b.name))
}

/// Sort siblings once per sample; keep zero-valued leaves for process inspection.
fn limited(mut nodes: Vec<Node>, parent: &str) -> Vec<Node> {
    nodes.sort_by(order);
    let count = nodes.partition_point(|n| n.value > 0.0);
    if count <= 8 {
        return nodes;
    }
    let hidden = nodes.split_off(count);
    let mut visible = nodes.into_iter();
    fn chunk(nodes: &mut std::vec::IntoIter<Node>, parent: &str) -> Vec<Node> {
        let mut head: Vec<_> = nodes.by_ref().take(8).collect();
        let remaining = nodes.len();
        if remaining > 0 {
            let id = format!("remainder:{parent}");
            let children = chunk(nodes, &id);
            head.push(Node {
                id,
                name: format!("{remaining} more processes"),
                value: children.iter().map(|n| n.value).sum(),
                process: None,
                children,
            });
        }
        head
    }
    let mut result = chunk(&mut visible, parent);
    result.extend(hidden);
    result
}

pub fn roots(mut groups: Vec<Node>) -> Vec<Node> {
    groups.retain(|n| n.value > 0.0);
    groups.sort_by(order);
    if groups.len() > 14 {
        let rest = groups.split_off(14);
        groups.push(Node::group("other".into(), "Other processes".into(), rest));
    }
    groups
}
