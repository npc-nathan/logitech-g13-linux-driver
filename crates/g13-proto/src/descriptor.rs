//! The HID report descriptor, and what it says about this device's reports.
//!
//! Written from the USB HID specification (Device Class Definition for HID, §6.2.2) and from the bytes
//! this device actually returns for a `GET_DESCRIPTOR(Report)` request. No part of it comes from any
//! other driver.
//!
//! The descriptor is the device telling us its own protocol: which usages appear in which report, at
//! which bit, in which direction. Parsing it is how this project learns the wire format without being
//! told by anybody's source code.

/// One item from the descriptor, decoded far enough to describe the reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Which of the four kinds this item is.
    pub kind: ItemKind,
    /// The item's own bytes as they arrived, without the prefix byte. Nothing is decoded out of them
    /// here - `parse` reads them as it walks.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// The four kinds of item a descriptor is made of: the high nibble of the prefix byte says which.
pub enum ItemKind {
    /// 0x04 …: usage page, logical range, report size and count, and so on.
    Global,
    /// 0x08 …: usage, usage minimum/maximum and the other per-field facts.
    Local,
    /// 0x0C …: collection start and end.
    Main,
    /// 0x10 …: reserved for the implementation.
    Reserved,
}

/// A field as the descriptor describes it: a run of bits inside a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// Report id this field belongs to; 0 when the descriptor declares none.
    pub report_id: u8,
    /// The main item that produced it.
    pub kind: MainItem,
    /// Bit offset inside its report.
    pub bit_offset: u32,
    /// Bits per field.
    pub report_size: u32,
    /// How many fields in the run.
    pub report_count: u32,
    /// Usage page in force when the field was declared.
    pub usage_page: u16,
    /// Usage ids in the run: an explicit usage, or the range from usage minimum to maximum.
    pub usages: Vec<u32>,
    /// True when the fields are a constant padding array rather than something meaningful.
    pub constant: bool,
    /// True for relative (movement) fields, false for absolute ones.
    pub relative: bool,
    /// True when the fields are an array of usage codes rather than a bitmap of usages.
    pub array: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// The main items: the ones that declare a field, or open and close a group of them.
pub enum MainItem {
    /// A field the device sends to the host.
    Input,
    /// A field the host sends to the device: the screen, the backlight, and the M keys' LEDs.
    Output,
    /// A field neither side sends on its own: parameters a driver may read or set.
    Feature,
    /// A group of fields, opened.
    Collection,
    /// The end of the innermost group.
    EndCollection,
}

/// The whole descriptor, decoded.
#[derive(Debug, Clone, Default)]
pub struct Descriptor {
    /// Every item, in the order the descriptor lists them.
    pub items: Vec<Item>,
    /// Input, output and feature fields in the order they appear.
    pub fields: Vec<Field>,
    /// Every usage page named, in order, for a readable summary.
    pub usage_pages: Vec<u16>,
}

impl Descriptor {
    /// Fields for a given report id and direction, in bit order.
    pub fn fields_for(&self, report_id: u8, kind: MainItem) -> Vec<&Field> {
        self.fields
            .iter()
            .filter(|field| field.report_id == report_id && field.kind == kind)
            .collect()
    }

    /// The size in bits of a report, taken as the highest end of any field in it.
    pub fn report_bits(&self, report_id: u8, kind: MainItem) -> u32 {
        self.fields_for(report_id, kind)
            .iter()
            .map(|field| field.bit_offset + field.report_size * field.report_count)
            .max()
            .unwrap_or(0)
    }

    /// A decoded, human-readable listing. Used by `g13 device info` and by the tests.
    pub fn describe(&self) -> Vec<String> {
        let mut out = Vec::new();
        for field in &self.fields {
            let usages = if field.usages.is_empty() {
                String::new()
            } else if field.usages.len() == 1 {
                format!(" usage={:#06x}", field.usages[0])
            } else {
                format!(
                    " usages={:#06x}..={:#06x}",
                    field.usages[0],
                    field.usages[field.usages.len() - 1]
                )
            };
            let note = if field.constant {
                " [padding]"
            } else if field.relative {
                " [relative]"
            } else {
                ""
            };
            out.push(format!(
                "report {:#04x} {:?}: bits {}..{} size {} count {}{} page {:#06x}{}{}",
                field.report_id,
                field.kind,
                field.bit_offset,
                field.bit_offset + field.report_size * field.report_count,
                field.report_size,
                field.report_count,
                usages,
                field.usage_page,
                note,
                if field.constant && field.usages.len() > field.report_count as usize {
                    " (usages truncated)"
                } else {
                    ""
                }
            ));
        }
        out
    }
}

/// Where the parser is while walking the descriptor.
#[derive(Debug, Default, Clone)]
struct State {
    /// Global usage page in force: the high 16 bits of every usage id collected after it.
    usage_page: u16,
    /// Global report size in force: bits per field in the next field-declaring main item.
    report_size: u32,
    /// Global report count in force: how many fields that main item declares.
    report_count: u32,
    /// Global report id in force, 0 until the descriptor names one, and the report the next field is in.
    report_id: u8,
    /// Local usage minimum: the bottom of the range `usage_maximum` closes, when a range is declared at all.
    usage_minimum: Option<u32>,
    /// Local usage maximum: the top of that range, the two expanded into one usage per code.
    usage_maximum: Option<u32>,
    /// Local usages gathered for the next main item, page-prefixed; every main item clears them.
    usages: Vec<u32>,
}

/// Parse a report descriptor into its items and fields.
///
/// The descriptor is a sequence of items, each starting with a prefix byte: tag in the high nibble,
/// type in bits 2-3, length code in the low two bits  -  where 3 means four bytes rather than three.
/// Global items persist until changed, local items are cleared by every main item, and main items are
/// where a report field is actually declared.
pub fn parse(descriptor: &[u8]) -> Descriptor {
    let mut parsed = Descriptor::default();
    let mut state = State::default();
    let mut index = 0usize;
    // bit offset of the next field, per report id and direction
    let mut offsets: std::collections::HashMap<(u8, MainItem), u32> =
        std::collections::HashMap::new();

    while index < descriptor.len() {
        let prefix = descriptor[index];
        index += 1;

        // 0xFE introduces a long item; nothing here needs one, so skip it honestly rather than guess.
        if prefix == 0xFE {
            if index + 2 <= descriptor.len() {
                let length = descriptor[index] as usize;
                index += 2 + length;
            }
            continue;
        }

        let size = match prefix & 0x03 {
            0 => 0usize,
            1 => 1,
            2 => 2,
            _ => 4,
        };
        let kind = match (prefix >> 2) & 0x03 {
            0 => ItemKind::Main,
            1 => ItemKind::Global,
            2 => ItemKind::Local,
            _ => ItemKind::Reserved,
        };
        let tag = (prefix >> 4) & 0x0F;

        if index + size > descriptor.len() {
            break;
        }
        let data = descriptor[index..index + size].to_vec();
        index += size;
        let value = match size {
            0 => 0u32,
            1 => data[0] as u32,
            2 => u16::from_le_bytes([data[0], data[1]]) as u32,
            _ => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        };
        parsed.items.push(Item {
            kind,
            data: data.clone(),
        });

        if kind == ItemKind::Global {
            match tag {
                0 => state.usage_page = value as u16,
                7 => state.report_size = value,
                8 => state.report_id = value as u8,
                9 => state.report_count = value,
                _ => {}
            }
            continue;
        }

        if kind == ItemKind::Local {
            match tag {
                0 => state.usages.push((state.usage_page as u32) << 16 | value),
                1 => state.usage_minimum = Some(value),
                2 => state.usage_maximum = Some(value),
                _ => {}
            }
            continue;
        }

        if kind != ItemKind::Main {
            continue;
        }

        match tag {
            10 | 12 => {
                // Collection start and end: nothing to record, structure only.
            }
            8 | 9 | 11 => {
                // Input, Output, Feature: a field is declared here.
                let main = match tag {
                    8 => MainItem::Input,
                    9 => MainItem::Output,
                    _ => MainItem::Feature,
                };

                // 0x02 is the "variable" flag: without it the fields are an array of usages rather
                // than a bitmap, which matters for how they are read.
                let variable = value & 0x0002 != 0;
                let constant = value & 0x0001 != 0;
                let relative = value & 0x0004 != 0;

                let mut usages: Vec<u32> = Vec::new();
                if !state.usages.is_empty() {
                    usages.clone_from(&state.usages);
                } else if let (Some(minimum), Some(maximum)) =
                    (state.usage_minimum, state.usage_maximum)
                {
                    if maximum >= minimum {
                        // Cap the expansion: a malformed descriptor must not become a memory event.
                        let count = (maximum - minimum + 1).min(256);
                        for offset in 0..count {
                            usages.push((state.usage_page as u32) << 16 | (minimum + offset));
                        }
                    }
                }

                let key = (state.report_id, main);
                let bit_offset = *offsets.get(&key).unwrap_or(&0);
                let bits = state.report_size * state.report_count;
                offsets.insert(key, bit_offset + bits);

                parsed.fields.push(Field {
                    report_id: state.report_id,
                    kind: main,
                    bit_offset,
                    report_size: state.report_size,
                    report_count: state.report_count,
                    usage_page: state.usage_page,
                    usages,
                    constant,
                    relative,
                    array: !variable,
                });
            }
            _ => {}
        }

        // Every main item clears the local state  -  including Collection and End Collection, which is
        // what stops a collection's own usage leaking into the fields declared inside it.
        state.usages.clear();
        state.usage_minimum = None;
        state.usage_maximum = None;
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but realistic descriptor: a 16-bit input bitmap in report 1, and a 1-byte id.
    fn sample() -> Vec<u8> {
        vec![
            0x05, 0x01, // usage page (generic desktop)
            0x09, 0x06, // usage (keyboard)
            0xA1, 0x01, // collection (application)
            0x85, 0x01, // report id 1
            0x75, 0x01, // report size 1
            0x95, 0x10, // report count 16
            0x05, 0x09, // usage page (button)
            0x19, 0x01, // usage minimum 1
            0x29, 0x10, // usage maximum 16
            0x81, 0x02, // input (data, variable, absolute)
            0x75, 0x08, // report size 8
            0x95, 0x02, // report count 2
            0x81, 0x01, // input (constant) - padding
            0xC0, // end collection
        ]
    }

    #[test]
    fn reads_report_size_count_and_id() {
        let parsed = parse(&sample());
        let inputs = parsed.fields_for(1, MainItem::Input);
        assert_eq!(inputs.len(), 2, "two input items");
        assert_eq!(inputs[0].report_size, 1);
        assert_eq!(inputs[0].report_count, 16);
        assert_eq!(inputs[0].bit_offset, 0);
        assert_eq!(
            inputs[1].bit_offset, 16,
            "padding starts after the 16 buttons"
        );
        assert!(inputs[1].constant, "second input item is padding");
    }

    #[test]
    fn expands_a_usage_range() {
        let parsed = parse(&sample());
        let buttons = parsed.fields_for(1, MainItem::Input);
        assert_eq!(buttons[0].usages.len(), 16);
        assert_eq!(buttons[0].usages[0], 0x0009_0001, "usage page 9, usage 1");
        assert_eq!(buttons[0].usages[15], 0x0009_0010);
    }

    #[test]
    fn reports_the_size_of_a_report() {
        let parsed = parse(&sample());
        assert_eq!(parsed.report_bits(1, MainItem::Input), 32);
    }

    #[test]
    fn a_truncated_descriptor_ends_quietly_rather_than_panicking() {
        let mut truncated = sample();
        truncated.truncate(truncated.len() - 3);
        let parsed = parse(&truncated);
        assert!(!parsed.fields.is_empty(), "parses what it can");
    }

    #[test]
    fn an_absurd_usage_range_does_not_explode() {
        let mut wild = sample();
        // usage maximum far beyond the minimum: must be capped, not allocated
        wild[13] = 0xFF;
        wild[14] = 0xFF;
        let parsed = parse(&wild);
        assert!(parsed.fields_for(1, MainItem::Input)[0].usages.len() <= 256);
    }

    #[test]
    fn output_fields_are_found_for_the_lcd_pipe() {
        let mut descriptor = sample();
        descriptor.extend_from_slice(&[
            0x85, 0x02, // report id 2
            0x75, 0x08, // size 8
            0x95, 0x40, // count 64
            0x91, 0x02, // output (data, variable)
        ]);
        let parsed = parse(&descriptor);
        let outputs = parsed.fields_for(2, MainItem::Output);
        assert_eq!(outputs.len(), 1);
        assert_eq!(parsed.report_bits(2, MainItem::Output), 512, "64 bytes");
    }
}
