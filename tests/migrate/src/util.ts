import type {JsonArray, JsonObject} from '@blake.regalia/belt';

import {is_array, is_object, keys} from '@blake.regalia/belt';

export function canonicalize_json<w_type extends JsonObject | JsonArray>(w_json: w_type): w_type {
	// recursively canonicalize arrays
	if(is_array(w_json)) return w_json.map(w => is_object(w)? canonicalize_json(w): w) as w_type;

	// build new json object for canonicalizing
	const h_test: JsonObject = {};

	// each lexicographically sorted key
	for(const si_key of keys(w_json as unknown as JsonObject).sort()) {
		// ref its value
		const w_value = (w_json as unknown as JsonObject)[si_key];

		// set value on test object
		h_test[si_key] = is_object(w_value)? canonicalize_json(w_value): w_value;
	}

	// return canonicalized object
	return h_test as w_type;
}
