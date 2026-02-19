import { TestBed } from '@angular/core/testing';

import { Audio } from './audio';
import { provideZonelessChangeDetection } from '@angular/core';

describe('Audio', () => {
  let service: Audio;

  beforeEach(() => {
    TestBed.configureTestingModule({
      providers: [provideZonelessChangeDetection()],
    });
    service = TestBed.inject(Audio);
  });

  it('should be created', () => {
    expect(service).toBeTruthy();
  });

  it('should have reactive subjects for audio events', () => {
    expect(service.padFinished$).toBeDefined();
    expect(service.fadeOutComplete$).toBeDefined();
  });
});
