use cosmwasm_std::{Api, Response};

pub struct GasTracker<'a> {
    logs: Vec<(String, String)>,
    api: &'a dyn Api,
}

impl<'a> GasTracker<'a> {
    pub fn new(api: &'a dyn Api) -> Self {
        Self {
            logs: Vec::new(),
            api,
        }
    }

    pub fn group<'b>(&'b mut self, name: String) -> GasGroup<'a, 'b> {
        GasGroup::new(self, name)
    }

    pub fn add_to_response(self, resp: Response) -> Response {
        let mut new_resp = resp.clone();
        for log in self.logs.into_iter() {
            new_resp = new_resp.add_attribute_plaintext(
                log.0,
                log.1
            );
        }
        new_resp
    }
}

pub struct GasGroup<'a, 'b> {
    tracker: &'b mut GasTracker<'a>,
    name: String,
    index: usize,
}

impl<'a, 'b> GasGroup<'a, 'b> {
    fn new(tracker: &'b mut GasTracker<'a>, name: String) -> Self {
        Self {
            tracker,
            name,
            index: 0,
        }
    }
    
    pub fn mark(&mut self) {
        self.log("");
    }

    pub fn log(&mut self, comment: &str) {
        let gas = self.tracker.api.check_gas();
        let log_entry = (
            format!(
                "group.{}.{}#{}",
                self.name,
                self.index,
                comment
            ),
            gas.unwrap_or(0u64).to_string()
        );
        self.tracker.logs.push(log_entry);
        self.index += 1;
    }
}
