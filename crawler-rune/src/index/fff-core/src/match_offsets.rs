use smallvec::SmallVec;

pub(crate) fn char_indices_to_byte_offsets(
    line: &str,
    char_indices: &[u32],
) -> SmallVec<[(u32, u32); 4]> {
    debug_assert!(char_indices.windows(2).all(|pair| pair[0] <= pair[1]));
    let mut result: SmallVec<[(u32, u32); 4]> = SmallVec::new();
    let mut chars = line.char_indices().enumerate().peekable();

    for &index in char_indices {
        while chars.peek().is_some_and(|&(i, _)| i < index as usize) {
            chars.next();
        }
        let Some(&(_, (start, ch))) = chars.peek() else {
            break;
        };
        let end = (start + ch.len_utf8()) as u32;
        if let Some(last) = result.last_mut()
            && last.1 == start as u32
        {
            last.1 = end;
        } else {
            result.push((start as u32, end));
        }
    }

    result
}

