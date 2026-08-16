use super::arena::{Arena, id::AttributeId};
use crate::Attribute;

impl<'html> Arena<Attribute<'html>, AttributeId> {
    pub(super) fn attribute_slice_to_range(
        &self,
        attributes: &[Attribute<'html>],
    ) -> Option<std::ops::Range<u32>> {
        if self.is_empty() || attributes.is_empty() {
            return None;
        }

        let tape_pointer_range = self.as_ptr_range();
        let slice_ptr = attributes.as_ptr();
        assert!(
            tape_pointer_range.start == slice_ptr || tape_pointer_range.contains(&slice_ptr),
            "attribute slice must belong to the persistent tape"
        );

        let start = unsafe { slice_ptr.offset_from_unsigned(tape_pointer_range.start) };
        let end = start + attributes.len();
        assert!(self.len() >= end);
        Some(start as u32..end as u32)
    }
}
