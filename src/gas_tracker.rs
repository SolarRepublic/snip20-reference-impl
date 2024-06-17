use cosmwasm_std::{Api, Response, StdResult};

pub struct GasTracker<'a> {
    api: &'a dyn Api,
    pub groups: Vec<GasGroup>,
}

pub struct GasGroup {
    pub name: String,
    pub logs: Vec<GasLog>, 
}

pub struct GasLog {
    pub value: u64,
    pub comment: String,
}

impl<'a> GasTracker<'a> {
    pub fn new(api: &'a dyn Api, group_name: &str) -> Self {
        GasTracker {
            api,
            groups: vec![
                GasGroup {
                    name: group_name.to_string(),
                    logs: vec![],
                }
            ],
        }
    }

    pub fn new_group(&mut self, group_name: &str) {
        self.groups.push(
            GasGroup {
                name: group_name.to_string(),
                logs: vec![],
            }
        );
    }

    pub fn log(&mut self, comment: &str) -> StdResult<()> {
        let current_group_idx = self.groups.len() - 1;
        //let logs_len = self.groups[current_group_idx].logs.len();
        let gas = self.api.check_gas()?;
        self.groups[current_group_idx].logs.push(
            GasLog {
                //key: format!("gas.{}.{}#{}", self.group_name, logs_len, comment),
                value: gas,
                comment: comment.to_string(),
            }
        );
        Ok(())
    }

    pub fn add_to_response(self, resp: Response) -> Response {
        let mut new_resp = resp.clone();
        for group in self.groups {
            for (idx, log) in group.logs.into_iter().enumerate() {
                new_resp = new_resp.add_attribute_plaintext(
                    format!("gas.{}.{}#{}", group.name, idx, log.comment),
                    format!("{}", log.value),
                );
            }
        }
        new_resp
    }
}