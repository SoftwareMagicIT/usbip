use super::*;

#[derive(Clone, Debug, Default)]
pub struct UsbDevice {
    pub path: String,
    pub bus_id: String,
    pub bus_num: u32,
    pub dev_num: u32,
    pub speed: u32,
    pub vendor_id: u16,
    pub product_id: u16,
    pub device_bcd: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub configuration_value: u8,
    pub num_configurations: u8,
    pub interfaces: Vec<UsbInterface>,
    pub ep0_in: UsbEndpoint,
    pub ep0_out: UsbEndpoint,
    // strings
    pub string_pool: HashMap<u8, String>,
    pub string_configuration: u8,
    pub string_manufacturer: u8,
    pub string_product: u8,
    pub string_serial: u8,
}

impl UsbDevice {
    pub fn new(index: u32) -> Self {
        let mut res = Self {
            path: format!("/sys/device/usbip/{}", index),
            bus_id: format!("{}", index),
            dev_num: index,
            speed: UsbSpeed::High as u32,
            ep0_in: UsbEndpoint {
                address: 0x80,
                attributes: EndpointAttributes::Control as u8,
                max_packet_size: EP0_MAX_PACKET_SIZE,
                interval: 0,
            },
            ep0_out: UsbEndpoint {
                address: 0x00,
                attributes: EndpointAttributes::Control as u8,
                max_packet_size: EP0_MAX_PACKET_SIZE,
                interval: 0,
            },
            // configured by default
            configuration_value: 1,
            num_configurations: 1,
            ..Self::default()
        };
        res.string_configuration = res.new_string("Default Configuration");
        res.string_manufacturer = res.new_string("Manufacturer");
        res.string_product = res.new_string("Product");
        res.string_serial = res.new_string("Serial");
        res
    }

    pub fn with_interface(
        mut self,
        interface_class: u8,
        interface_subclass: u8,
        interface_protocol: u8,
        name: &str,
        endpoints: Vec<UsbEndpoint>,
        class_specific_descriptor: Vec<u8>,
    ) -> Self {
        let string_interface = self.new_string(name);
        self.interfaces.push(UsbInterface {
            interface_class,
            interface_subclass,
            interface_protocol,
            endpoints,
            string_interface,
            class_specific_descriptor,
        });
        self
    }

    pub(crate) fn new_string(&mut self, s: &str) -> u8 {
        for i in 1.. {
            if self.string_pool.get(&i).is_none() {
                self.string_pool.insert(i, s.to_string());
                return i;
            }
        }
        panic!("string poll exhausted")
    }

    pub(crate) fn find_ep(&self, ep: u8) -> Option<UsbEndpoint> {
        if ep == self.ep0_in.address {
            Some(self.ep0_in)
        } else if ep == self.ep0_out.address {
            Some(self.ep0_out)
        } else {
            for intf in &self.interfaces {
                for endpoint in &intf.endpoints {
                    if endpoint.address == ep {
                        return Some(*endpoint);
                    }
                }
            }
            None
        }
    }

    pub(crate) async fn write_dev(&self, socket: &mut TcpStream) -> Result<()> {
        socket_write_fixed_string(socket, &self.path, 256).await?;
        socket_write_fixed_string(socket, &self.bus_id, 32).await?;

        // fields
        socket.write_u32(self.bus_num).await?;
        socket.write_u32(self.dev_num).await?;
        socket.write_u32(self.speed).await?;
        socket.write_u16(self.vendor_id).await?;
        socket.write_u16(self.product_id).await?;
        socket.write_u16(self.device_bcd).await?;
        socket.write_u8(self.device_class).await?;
        socket.write_u8(self.device_subclass).await?;
        socket.write_u8(self.device_protocol).await?;
        socket.write_u8(self.configuration_value).await?;
        socket.write_u8(self.num_configurations).await?;
        socket.write_u8(self.interfaces.len() as u8).await?;

        Ok(())
    }

    pub(crate) async fn write_dev_with_interfaces(&self, socket: &mut TcpStream) -> Result<()> {
        self.write_dev(socket).await?;

        for interface in &self.interfaces {
            socket.write_u8(interface.interface_class).await?;
            socket.write_u8(interface.interface_subclass).await?;
            socket.write_u8(interface.interface_protocol).await?;
            // padding
            socket.write_u8(0).await?;
        }
        Ok(())
    }

    pub(crate) async fn handle_urb(
        &self,
        socket: &mut TcpStream,
        ep: UsbEndpoint,
        transfer_buffer_length: u32,
        setup: [u8; 8],
    ) -> Result<Vec<u8>> {
        use DescriptorType::*;
        use Direction::*;
        use EndpointAttributes::*;
        use StandardRequest::*;

        // parse setup
        let request_type = setup[0];
        let request = setup[1];
        let value = (setup[3] as u16) << 8 | (setup[2] as u16);
        let index = (setup[5] as u16) << 8 | (setup[4] as u16);
        let length = (setup[7] as u16) << 8 | (setup[6] as u16);

        match (FromPrimitive::from_u8(ep.attributes), ep.direction()) {
            (Some(Control), In) => {
                // control in
                debug!("Control IN bmRequestType={:b} bRequest={:x} wValue={:x} wIndex={:x} wLength={:x}", request_type, request, value, index, length);
                match (request_type, FromPrimitive::from_u8(request)) {
                    (0b10000000, Some(GetDescriptor)) => {
                        // high byte: type
                        match FromPrimitive::from_u16(value >> 8) {
                            Some(Device) => {
                                debug!("Get device descriptor");
                                let mut desc = vec![
                                    0x12,         // bLength
                                    Device as u8, // bDescriptorType: Device
                                    0x10,
                                    0x02,                      // bcdUSB: USB 2.1
                                    self.device_class,         // bDeviceClass
                                    self.device_subclass,      // bDeviceSubClass
                                    self.device_protocol,      // bDeviceProtocol
                                    EP0_MAX_PACKET_SIZE as u8, // bMaxPacketSize0
                                    self.vendor_id as u8,      // idVendor
                                    (self.vendor_id >> 8) as u8,
                                    self.product_id as u8, // idProduct
                                    (self.product_id >> 8) as u8,
                                    self.device_bcd as u8, // bcdDevice
                                    (self.device_bcd >> 8) as u8,
                                    self.string_manufacturer, // iManufacturer
                                    self.string_product,      // iProduct
                                    self.string_serial,       // iSerial
                                    self.num_configurations,
                                ];

                                // requested len too short: wLength < real length
                                if length < desc.len() as u16 {
                                    desc.resize(length as usize, 0);
                                }
                                return Ok(desc);
                            }
                            Some(BOS) => {
                                debug!("Get BOS descriptor");
                                let mut desc = vec![
                                    0x05,      // bLength
                                    BOS as u8, // bDescriptorType: BOS
                                    0x05, 0x00, // wTotalLength
                                    0x00, // bNumCapabilities
                                ];

                                // requested len too short: wLength < real length
                                if length < desc.len() as u16 {
                                    desc.resize(length as usize, 0);
                                }
                                return Ok(desc);
                            }
                            Some(Configuration) => {
                                debug!("Get configuration descriptor");
                                let mut desc = vec![
                                    0x09,                // bLength
                                    Configuration as u8, // bDescriptorType: Configuration
                                    0x00,
                                    0x00, // wTotalLength: to be filled below
                                    self.interfaces.len() as u8, // bNumInterfaces
                                    self.configuration_value, // bConfigurationValue
                                    self.string_configuration, // iConfiguration
                                    0x80, // bmAttributes Bus Powered
                                    0x32, // bMaxPower 100mA
                                ];
                                for (i, intf) in self.interfaces.iter().enumerate() {
                                    let mut intf_desc = vec![
                                        0x09,                       // bLength
                                        Interface as u8,            // bDescriptorType: Interface
                                        i as u8,                    // bInterfaceNum
                                        0x00,                       // bAlternateSettings
                                        intf.endpoints.len() as u8, // bNumEndpoints
                                        intf.interface_class,       // bInterfaceClass
                                        intf.interface_subclass,    // bInterfaceSubClass
                                        intf.interface_protocol,    // bInterfaceProtocol
                                        intf.string_interface,      //iInterface
                                    ];
                                    // class specific endpoint
                                    let mut specific = intf.class_specific_descriptor.clone();
                                    intf_desc.append(&mut specific);
                                    // endpoint descriptors
                                    for endpoint in &intf.endpoints {
                                        let mut ep_desc = vec![
                                            0x07,                // bLength
                                            Endpoint as u8,      // bDescriptorType: Endpoint
                                            endpoint.address,    // bEndpointAddress
                                            endpoint.attributes, // bmAttributes
                                            endpoint.max_packet_size as u8,
                                            (endpoint.max_packet_size >> 8) as u8, // wMaxPacketSize
                                            endpoint.interval,                     // bInterval
                                        ];
                                        intf_desc.append(&mut ep_desc);
                                    }
                                    desc.append(&mut intf_desc);
                                }
                                // length
                                let len = desc.len() as u16;
                                desc[2] = len as u8;
                                desc[3] = (len >> 8) as u8;

                                // requested len too short: wLength < real length
                                if length < desc.len() as u16 {
                                    desc.resize(length as usize, 0);
                                }
                                return Ok(desc);
                            }
                            Some(String) => {
                                let index = value as u8;
                                if index == 0 {
                                    // language ids
                                    let mut desc = vec![
                                        4,                            // bLength
                                        DescriptorType::String as u8, // bDescriptorType
                                        0x09,
                                        0x04, // bLANGID, en-US
                                    ];
                                    // requested len too short: wLength < real length
                                    if length < desc.len() as u16 {
                                        desc.resize(length as usize, 0);
                                    }
                                    return Ok(desc);
                                } else {
                                    let s = &self.string_pool[&index];
                                    let bytes: Vec<u16> = s.encode_utf16().collect();
                                    let mut desc = vec![
                                        (2 + bytes.len() * 2) as u8,  // bLength
                                        DescriptorType::String as u8, // bDescriptorType
                                    ];
                                    for byte in bytes {
                                        desc.push(byte as u8);
                                        desc.push((byte >> 8) as u8);
                                    }

                                    // requested len too short: wLength < real length
                                    if length < desc.len() as u16 {
                                        desc.resize(length as usize, 0);
                                    }
                                    return Ok(desc);
                                }
                            }
                            _ => unimplemented!("desc type"),
                        }
                    }
                    _ => unimplemented!("control in"),
                }
            }
            _ => unimplemented!("transfer to {:?}", ep),
        }
        Ok(vec![])
    }
}