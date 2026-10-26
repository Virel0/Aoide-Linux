//! The output picker's lines: the system default, then every device, and
//! the chosen one found again after the system renumbers its cards.

/// One line of the output picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceOption {
    pub label: String,
    pub choice: aoide_audio::DeviceChoice,
}

impl std::fmt::Display for DeviceOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

/// The output picker's lines: the system default (naming the device it is
/// now, when the list says), then every listed device, then the chosen one
/// if it isn't connected, so the picker still shows what was chosen.
pub fn device_options(
    listed: &[aoide_audio::OutputDevice],
    chosen: &aoide_audio::DeviceChoice,
) -> Vec<DeviceOption> {
    use aoide_audio::DeviceChoice;
    let default = listed.iter().find(|d| d.is_default);
    let mut options = vec![DeviceOption {
        label: match default {
            Some(device) => format!("System default ({})", device.name),
            None => "System default".to_owned(),
        },
        choice: DeviceChoice::FollowDefault,
    }];
    options.extend(listed.iter().map(|device| DeviceOption {
        label: device.name.clone(),
        choice: DeviceChoice::from(device),
    }));
    if let DeviceChoice::Device { name, .. } = chosen
        && !listed.iter().any(|d| same_device(chosen, d))
    {
        options.push(DeviceOption {
            label: format!("{name} (not connected)"),
            choice: chosen.clone(),
        });
    }
    options
}

/// Whether `choice` names `device`: by id, or by name when the id has
/// changed (as `aoide_audio::DeviceChoice` finds it).
pub fn same_device(choice: &aoide_audio::DeviceChoice, device: &aoide_audio::OutputDevice) -> bool {
    match choice {
        aoide_audio::DeviceChoice::FollowDefault => false,
        aoide_audio::DeviceChoice::Device { id, name } => *id == device.id || *name == device.name,
    }
}

/// The picker's line for what is chosen.
pub fn chosen_option(
    options: &[DeviceOption],
    chosen: &aoide_audio::DeviceChoice,
) -> Option<DeviceOption> {
    options
        .iter()
        .find(|o| o.choice == *chosen)
        .or_else(|| {
            let aoide_audio::DeviceChoice::Device { id, name } = chosen else {
                return None;
            };
            options.iter().find(|o| match &o.choice {
                aoide_audio::DeviceChoice::Device { id: i, name: n } => i == id || n == name,
                aoide_audio::DeviceChoice::FollowDefault => false,
            })
        })
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(id: &str, name: &str, is_default: bool) -> aoide_audio::OutputDevice {
        aoide_audio::OutputDevice {
            id: id.into(),
            name: name.into(),
            is_default,
        }
    }

    #[test]
    fn the_output_picker_lists_the_default_then_every_device() {
        use aoide_audio::DeviceChoice;
        let listed = [
            device("alsa:pipewire", "PipeWire", true),
            device("alsa:hw:CARD=USB", "USB DAC", false),
        ];
        let options = device_options(&listed, &DeviceChoice::FollowDefault);
        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["System default (PipeWire)", "PipeWire", "USB DAC"]);
        assert_eq!(
            chosen_option(&options, &DeviceChoice::FollowDefault)
                .unwrap()
                .label,
            "System default (PipeWire)"
        );
        let usb = DeviceChoice::from(&listed[1]);
        assert_eq!(chosen_option(&options, &usb).unwrap().label, "USB DAC");
    }

    #[test]
    fn a_renumbered_card_is_found_by_name_and_a_missing_one_still_shown() {
        use aoide_audio::DeviceChoice;
        let listed = [device("alsa:hw:CARD=2", "USB DAC", false)];
        let renumbered = DeviceChoice::Device {
            id: "alsa:hw:CARD=1".into(),
            name: "USB DAC".into(),
        };
        let options = device_options(&listed, &renumbered);
        assert_eq!(options.len(), 2);
        assert_eq!(
            chosen_option(&options, &renumbered).unwrap().label,
            "USB DAC"
        );

        let gone = DeviceChoice::Device {
            id: "alsa:hw:CARD=9".into(),
            name: "Headphones".into(),
        };
        let options = device_options(&listed, &gone);
        assert_eq!(options.last().unwrap().label, "Headphones (not connected)");
        assert_eq!(
            chosen_option(&options, &gone).unwrap().label,
            "Headphones (not connected)"
        );
        // Nothing listed at all: the default without a name.
        assert_eq!(
            device_options(&[], &DeviceChoice::FollowDefault)[0].label,
            "System default"
        );
    }
}
