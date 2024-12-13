import type {MigratedContractInterface} from './contract';
import type {SecretContract} from '@solar-republic/neutrino';

import {__UNDEFINED} from '@blake.regalia/belt';

export const G_GLOBAL = {
	k_snip_migrated: __UNDEFINED as unknown as SecretContract<MigratedContractInterface>,
};
