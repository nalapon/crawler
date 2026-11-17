use super::sampling::Process;
use std::collections::{HashMap, HashSet};
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
    pub processes: Vec<Process>,
}
impl Group {
    pub fn value(&self, metric: Metric) -> Option<f64> {
        sum(self.processes.iter().map(|p| metric.value(p)))
    }
}
pub fn sum(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    values.flatten().reduce(|a, b| a + b)
}
fn application(p: &Process) -> Option<String> {
    p.executable
        .find(".app/")
        .map(|i| p.executable[..i + 4].to_string())
}
pub fn groups(processes: &[Process], query: &str) -> Vec<Group> {
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
        if !matches(query, &format!("{name} {} {}", p.name, p.id.pid)) {
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

/// The Swift process filter's bounded bit-parallel fuzzy subsequence matcher.
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
    pub processes: Vec<Process>,
    pub child: bool,
}
impl ListRow {
    pub fn value(&self, m: Metric) -> Option<f64> {
        sum(self.processes.iter().map(|p| m.value(p)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: String,
    pub name: String,
    pub value: f64,
    pub processes: Vec<Process>,
    pub children: Vec<Node>,
}
pub fn forest(processes: &[Process], metric: Metric) -> Vec<Node> {
    let ids: HashSet<_> = processes.iter().map(|p| p.id.pid).collect();
    fn node(p: &Process, all: &[Process], m: Metric, seen: &mut HashSet<i32>) -> Node {
        seen.insert(p.id.pid);
        let mut children = Vec::new();
        let mut members = vec![p.clone()];
        for child in all
            .iter()
            .filter(|c| c.parent == p.id.pid && c.id.pid != p.id.pid)
        {
            if seen.contains(&child.id.pid) {
                continue;
            }
            let n = node(child, all, m, seen);
            members.extend(n.processes.iter().cloned());
            children.push(n);
        }
        let own = m.value(p).unwrap_or(0.0);
        let value = own + children.iter().map(|n| n.value).sum::<f64>();
        if !children.is_empty() {
            children.insert(
                0,
                Node {
                    id: format!("own:{}", p.id.key()),
                    name: p.name.clone(),
                    value: own,
                    processes: vec![p.clone()],
                    children: vec![],
                },
            );
        }
        Node {
            id: p.id.key(),
            name: p.name.clone(),
            value,
            processes: members,
            children,
        }
    }
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for p in processes
        .iter()
        .filter(|p| !ids.contains(&p.parent) || p.parent == p.id.pid)
    {
        out.push(node(p, processes, metric, &mut seen));
    }
    for p in processes {
        if !seen.contains(&p.id.pid) {
            out.push(node(p, processes, metric, &mut seen));
        }
    }
    out
}
pub fn limited(mut nodes: Vec<Node>, parent: &str) -> Vec<Node> {
    nodes.retain(|n| n.value > 0.0);
    nodes.sort_by(|a, b| {
        b.value
            .total_cmp(&a.value)
            .then_with(|| a.name.cmp(&b.name))
    });
    if nodes.len() > 8 {
        let tail = nodes.split_off(8);
        nodes.push(Node {
            id: format!("remainder:{parent}"),
            name: format!("{} more processes", tail.len()),
            value: tail.iter().map(|n| n.value).sum(),
            processes: tail
                .iter()
                .flat_map(|n| n.processes.iter().cloned())
                .collect(),
            children: tail,
        });
    }
    nodes
}
